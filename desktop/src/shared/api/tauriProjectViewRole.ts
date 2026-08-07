import type { ProjectView } from "@/shared/api/tauriProjectView";
import { ProjectViewIntegrityError } from "@/shared/api/tauriProjectViewIntegrity";
import {
  normalizeRoleBriefRoleDirectory,
  type ProjectRoleDirectory,
  type RawRoleBriefRoleDirectory,
  validateRoleBriefRoleDirectoryContinuity,
} from "@/shared/api/tauriProjectViewRoleDirectory";
import {
  normalizeCheckpoint,
  normalizeHandoff,
  type ProjectRoleCheckpoint,
  type ProjectRoleHandoff,
  type ProjectRoleContinuityReference,
  type RawProjectRoleCheckpoint,
  type RawProjectRoleHandoff,
} from "@/shared/api/tauriProjectViewRoleHistory";
import {
  type ProjectRoleBriefBaseContextV3,
  validateBaseRoleBriefV3,
} from "@/shared/api/tauriProjectViewRoleV3";

export type {
  ProjectRoleDirectory,
  ProjectRoleDirectoryEntry,
} from "@/shared/api/tauriProjectViewRoleDirectory";
export type {
  ProjectRoleCheckpoint,
  ProjectRoleCheckpointContent,
  ProjectRoleContinuityReference,
  ProjectRoleHandoff,
  ProjectRoleHandoffCause,
  ProjectRoleHandoffContent,
} from "@/shared/api/tauriProjectViewRoleHistory";
export {
  mutateProjectViewRole,
  serializeProjectViewRoleMutationIntent,
} from "@/shared/api/tauriProjectViewRoleMutation";
export type {
  ProjectViewRoleMutationIntent,
  ProjectViewRoleMutationResult,
  RawProjectViewRoleMutationResult,
} from "@/shared/api/tauriProjectViewRoleMutation";

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
  context_references?: unknown[];
};

export type RawProjectRoleProposal = {
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

export type RawProjectRoleAssignment = {
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
  project_view_schema_version: 3;
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
  role_directory: RawRoleBriefRoleDirectory;
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
  latest_checkpoint?: {
    checkpoint: RawProjectRoleCheckpoint;
    source: RawRoleBriefSource;
  };
  recent_handoffs: Array<{
    handoff: RawProjectRoleHandoff;
    source: RawRoleBriefSource;
  }>;
  source_revisions: {
    meta_event_id: string;
    meta_change_id: string;
    membership_event_id: string;
    project_updated_at: string;
    document_metadata?: unknown;
  };
  context?: unknown;
};

export type RawProjectViewRoleContinuity = {
  roles: RawProjectRoleDefinition[];
  proposals: RawProjectRoleProposal[];
  assignments: RawProjectRoleAssignment[];
  commitments: RawProjectWorkCommitment[];
  workResponsibilities: Array<{ workId: string; roleId: string }>;
  checkpoints: RawProjectRoleCheckpoint[];
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

export type ProjectRoleBrief = {
  schemaVersion: 3;
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
  roleDirectory: ProjectRoleDirectory;
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
  latestCheckpoint?: ProjectRoleCheckpoint;
  recentHandoffs: ProjectRoleHandoff[];
  sourceRevisions: {
    metaEventId: string;
    metaChangeId: string;
    membershipEventId: string;
    projectUpdatedAt: string;
  };
  baseContext: ProjectRoleBriefBaseContextV3;
};

export type ProjectViewRoleContinuity = {
  roles: ProjectRoleDefinition[];
  proposals: ProjectRoleProposal[];
  assignments: ProjectRoleAssignment[];
  commitments: ProjectWorkCommitment[];
  workResponsibilities: Array<{ workId: string; roleId: string }>;
  checkpoints: ProjectRoleCheckpoint[];
  handoffs: ProjectRoleHandoff[];
  members: Array<{ pubkey: string; role: ProjectCommunityMemberRole }>;
  briefs: ProjectRoleBrief[];
};

export function normalizeRoleProposal(
  proposal: RawProjectRoleProposal,
): ProjectRoleProposal {
  return {
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
  };
}

export function normalizeRoleAssignment(
  assignment: RawProjectRoleAssignment,
): ProjectRoleAssignment {
  return {
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
  };
}

function validateContinuityReferences(
  references: ProjectRoleContinuityReference[],
  objectIds: Set<string>,
  assignments: Map<string, ProjectRoleAssignment>,
  commitments: Map<string, ProjectWorkCommitment>,
  partialHistory = false,
) {
  for (const reference of references) {
    const valid =
      reference.referenceType === "object"
        ? objectIds.has(reference.objectId)
        : reference.referenceType === "assignment"
          ? partialHistory || assignments.has(reference.assignmentId)
          : reference.referenceType === "commitment"
            ? partialHistory || commitments.has(reference.commitmentId)
            : /^[0-9a-f]{64}$/.test(reference.eventId);
    if (!valid) {
      throw new ProjectViewIntegrityError(
        "Role continuity history references missing Project state",
      );
    }
  }
}

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
  projectionGeneration: number,
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
  const baseContext = validateBaseRoleBriefV3(
    raw as unknown as Record<string, unknown>,
    projectRevision,
    projectionGeneration,
  );
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
  const roleDirectory = normalizeRoleBriefRoleDirectory(
    raw.role_directory,
    state,
  );
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
    schemaVersion: 3,
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
    roleDirectory,
    state,
    responsibleWork,
    relatedObjects: raw.related_objects.map((related) => ({
      id: related.object.id,
      objectType: related.object.object_type,
      title: briefObjectTitle(related.object),
    })),
    latestCheckpoint: raw.latest_checkpoint
      ? normalizeCheckpoint(raw.latest_checkpoint.checkpoint)
      : undefined,
    recentHandoffs: raw.recent_handoffs.map((handoff) =>
      normalizeHandoff(handoff.handoff),
    ),
    sourceRevisions: {
      metaEventId: raw.source_revisions.meta_event_id,
      metaChangeId: raw.source_revisions.meta_change_id,
      membershipEventId: raw.source_revisions.membership_event_id,
      projectUpdatedAt: raw.source_revisions.project_updated_at,
    },
    baseContext,
  };
}

export function normalizeRoleContinuity(
  raw: RawProjectViewRoleContinuity,
  view: ProjectView,
  projectRevision: number,
  projectionGeneration: number,
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
  const proposals = raw.proposals.map(normalizeRoleProposal);
  const assignments = raw.assignments.map(normalizeRoleAssignment);
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
  const checkpoints = raw.checkpoints.map(normalizeCheckpoint);
  const handoffs = raw.handoffs.map(normalizeHandoff);
  const briefs = raw.briefs.map((brief) =>
    normalizeRoleBrief(brief, projectRevision, projectionGeneration),
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
  const commitmentsById = new Map(
    commitments.map((commitment) => [commitment.commitmentId, commitment]),
  );
  const rolesById = new Map(roles.map((role) => [role.roleId, role]));
  const activeAssignmentsByRoleId = new Map(
    assignments
      .filter((assignment) => !assignment.endedAt)
      .map((assignment) => [assignment.roleId, assignment]),
  );
  const objectIds = new Set([
    view.profile.id,
    ...view.goals.flatMap((goal) => [
      goal.goal.id,
      ...goal.plans.flatMap((plan) => [
        plan.plan.id,
        ...plan.stages.flatMap((stage) => [
          stage.stage.id,
          ...stage.requirements.flatMap((requirement) => [
            requirement.requirement.id,
            ...requirement.works.map((work) => work.id),
          ]),
          ...stage.issues.flatMap((issue) => [
            issue.issue.id,
            ...issue.works.map((work) => work.id),
          ]),
        ]),
      ]),
    ]),
    ...view.unboundPlans.flatMap((plan) => [
      plan.plan.id,
      ...plan.stages.flatMap((stage) => [
        stage.stage.id,
        ...stage.requirements.flatMap((requirement) => [
          requirement.requirement.id,
          ...requirement.works.map((work) => work.id),
        ]),
        ...stage.issues.flatMap((issue) => [
          issue.issue.id,
          ...issue.works.map((work) => work.id),
        ]),
      ]),
    ]),
    ...view.unplannedRequirements.flatMap((requirement) => [
      requirement.requirement.id,
      ...requirement.works.map((work) => work.id),
    ]),
    ...view.unplannedIssues.flatMap((issue) => [
      issue.issue.id,
      ...issue.works.map((work) => work.id),
    ]),
    ...view.roles.map((role) => role.id),
    ...view.resources.map((resource) => resource.id),
  ]);
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
  const checkpointIds = new Set<string>();
  for (const checkpoint of checkpoints) {
    const assignment = assignmentsById.get(checkpoint.assignmentId);
    if (
      checkpointIds.has(checkpoint.checkpointId) ||
      (assignment &&
        (assignment.roleId !== checkpoint.roleId ||
          assignment.memberPubkey.toLowerCase() !==
            checkpoint.createdBy.toLowerCase())) ||
      checkpoint.basedOnProjectRevision >= checkpoint.projectRevision ||
      checkpoint.projectRevision > projectRevision
    ) {
      throw new ProjectViewIntegrityError(
        `Checkpoint ${checkpoint.checkpointId} has invalid attribution`,
      );
    }
    checkpointIds.add(checkpoint.checkpointId);
    validateContinuityReferences(
      checkpoint.content.references,
      objectIds,
      assignmentsById,
      commitmentsById,
      true,
    );
  }
  for (const checkpoint of checkpoints) {
    const superseded = checkpoint.supersedesCheckpointId
      ? checkpoints.find(
          (candidate) =>
            candidate.checkpointId === checkpoint.supersedesCheckpointId,
        )
      : undefined;
    if (
      superseded &&
      (superseded.roleId !== checkpoint.roleId ||
        superseded.assignmentId !== checkpoint.assignmentId ||
        superseded.projectRevision >= checkpoint.projectRevision)
    ) {
      throw new ProjectViewIntegrityError(
        `Checkpoint ${checkpoint.checkpointId} supersedes invalid history`,
      );
    }
  }
  for (const handoff of handoffs) {
    const source = assignmentsById.get(handoff.fromAssignmentId);
    const target = handoff.toAssignmentId
      ? assignmentsById.get(handoff.toAssignmentId)
      : undefined;
    const checkpoint = handoff.checkpointId
      ? checkpoints.find(
          (candidate) => candidate.checkpointId === handoff.checkpointId,
        )
      : undefined;
    if (
      (source && source.roleId !== handoff.roleId) ||
      (target && target.roleId !== handoff.roleId) ||
      (checkpoint &&
        (checkpoint.roleId !== handoff.roleId ||
          checkpoint.assignmentId !== handoff.fromAssignmentId)) ||
      handoff.affectedCommitmentIds.some((id) => {
        const commitment = commitmentsById.get(id);
        return (
          commitment && commitment.assignmentId !== handoff.fromAssignmentId
        );
      })
    ) {
      throw new ProjectViewIntegrityError(
        `Handoff ${handoff.handoffId} has invalid attribution`,
      );
    }
    validateContinuityReferences(
      handoff.content.references,
      objectIds,
      assignmentsById,
      commitmentsById,
      true,
    );
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
    validateRoleBriefRoleDirectoryContinuity(
      brief.roleDirectory,
      brief.state,
      roles.filter((role) => role.active).length,
      rolesById,
      activeAssignmentsByRoleId,
    );
    if (
      brief.latestCheckpoint &&
      !checkpointIds.has(brief.latestCheckpoint.checkpointId)
    ) {
      throw new ProjectViewIntegrityError(
        "Role Brief latest Checkpoint is absent from verified history",
      );
    }
    if (
      brief.recentHandoffs.some(
        (briefHandoff) =>
          !handoffs.some(
            (handoff) => handoff.handoffId === briefHandoff.handoffId,
          ),
      )
    ) {
      throw new ProjectViewIntegrityError(
        "Role Brief Handoff is absent from verified history",
      );
    }
  }
  return {
    roles,
    proposals,
    assignments,
    commitments,
    workResponsibilities,
    checkpoints,
    handoffs,
    members: raw.members,
    briefs,
  };
}
