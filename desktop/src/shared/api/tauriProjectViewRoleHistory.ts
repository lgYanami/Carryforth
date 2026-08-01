export type ProjectRoleHandoffCause =
  | "planned"
  | "revoked"
  | "replaced"
  | "unrecoverable"
  | "membership_ended"
  | "role_deactivated"
  | "other";

export type RawProjectRoleContinuityReference =
  | { reference_type: "object"; object_id: string; label?: string }
  | { reference_type: "assignment"; assignment_id: string; label?: string }
  | { reference_type: "commitment"; commitment_id: string; label?: string }
  | { reference_type: "nostr_event"; event_id: string; label?: string };

export type RawProjectRoleCheckpointContent = {
  summary: string;
  current_focus: string[];
  progress: string[];
  blockers: string[];
  risks: string[];
  open_questions: string[];
  next_steps: string[];
  references: RawProjectRoleContinuityReference[];
};

export type RawProjectRoleCheckpoint = {
  checkpoint_id: string;
  role_id: string;
  assignment_id: string;
  based_on_project_revision: number;
  content: RawProjectRoleCheckpointContent;
  supersedes_checkpoint_id?: string;
  created_by: string;
  created_at: string;
  entity_revision: number;
  project_revision: number;
};

export type RawProjectRoleHandoffContent = {
  summary?: string;
  unresolved_items: string[];
  references: RawProjectRoleContinuityReference[];
};

export type RawProjectRoleHandoff = {
  handoff_id: string;
  role_id: string;
  from_assignment_id: string;
  to_assignment_id?: string;
  checkpoint_id?: string;
  affected_commitment_ids: string[];
  content: RawProjectRoleHandoffContent;
  cause: ProjectRoleHandoffCause;
  system_generated: boolean;
  created_by?: string;
  created_at: string;
  entity_revision: number;
  project_revision: number;
};

export type ProjectRoleContinuityReference =
  | { referenceType: "object"; objectId: string; label?: string }
  | { referenceType: "assignment"; assignmentId: string; label?: string }
  | { referenceType: "commitment"; commitmentId: string; label?: string }
  | { referenceType: "nostr_event"; eventId: string; label?: string };

export type ProjectRoleCheckpointContent = {
  summary: string;
  currentFocus: string[];
  progress: string[];
  blockers: string[];
  risks: string[];
  openQuestions: string[];
  nextSteps: string[];
  references: ProjectRoleContinuityReference[];
};

export type ProjectRoleCheckpoint = {
  checkpointId: string;
  roleId: string;
  assignmentId: string;
  basedOnProjectRevision: number;
  content: ProjectRoleCheckpointContent;
  supersedesCheckpointId?: string;
  createdBy: string;
  createdAt: string;
  entityRevision: number;
  projectRevision: number;
};

export type ProjectRoleHandoffContent = {
  summary?: string;
  unresolvedItems: string[];
  references: ProjectRoleContinuityReference[];
};

export type ProjectRoleHandoff = {
  handoffId: string;
  roleId: string;
  fromAssignmentId: string;
  toAssignmentId?: string;
  checkpointId?: string;
  affectedCommitmentIds: string[];
  content: ProjectRoleHandoffContent;
  cause: ProjectRoleHandoffCause;
  systemGenerated: boolean;
  createdBy?: string;
  createdAt: string;
  entityRevision: number;
  projectRevision: number;
};

export function normalizeCheckpoint(
  checkpoint: RawProjectRoleCheckpoint,
): ProjectRoleCheckpoint {
  return {
    checkpointId: checkpoint.checkpoint_id,
    roleId: checkpoint.role_id,
    assignmentId: checkpoint.assignment_id,
    basedOnProjectRevision: checkpoint.based_on_project_revision,
    content: {
      summary: checkpoint.content.summary,
      currentFocus: checkpoint.content.current_focus,
      progress: checkpoint.content.progress,
      blockers: checkpoint.content.blockers,
      risks: checkpoint.content.risks,
      openQuestions: checkpoint.content.open_questions,
      nextSteps: checkpoint.content.next_steps,
      references: checkpoint.content.references.map(normalizeReference),
    },
    supersedesCheckpointId: checkpoint.supersedes_checkpoint_id,
    createdBy: checkpoint.created_by,
    createdAt: checkpoint.created_at,
    entityRevision: checkpoint.entity_revision,
    projectRevision: checkpoint.project_revision,
  };
}

export function normalizeHandoff(
  handoff: RawProjectRoleHandoff,
): ProjectRoleHandoff {
  return {
    handoffId: handoff.handoff_id,
    roleId: handoff.role_id,
    fromAssignmentId: handoff.from_assignment_id,
    toAssignmentId: handoff.to_assignment_id,
    checkpointId: handoff.checkpoint_id,
    affectedCommitmentIds: handoff.affected_commitment_ids,
    content: {
      summary: handoff.content.summary,
      unresolvedItems: handoff.content.unresolved_items,
      references: handoff.content.references.map(normalizeReference),
    },
    cause: handoff.cause,
    systemGenerated: handoff.system_generated,
    createdBy: handoff.created_by,
    createdAt: handoff.created_at,
    entityRevision: handoff.entity_revision,
    projectRevision: handoff.project_revision,
  };
}

export function serializeCheckpointContent(
  content: ProjectRoleCheckpointContent,
): Record<string, unknown> {
  return {
    summary: content.summary,
    current_focus: content.currentFocus,
    progress: content.progress,
    blockers: content.blockers,
    risks: content.risks,
    open_questions: content.openQuestions,
    next_steps: content.nextSteps,
    references: content.references.map(serializeReference),
  };
}

export function serializeHandoffContent(
  content: ProjectRoleHandoffContent,
): Record<string, unknown> {
  return {
    summary: content.summary,
    unresolved_items: content.unresolvedItems,
    references: content.references.map(serializeReference),
  };
}

function normalizeReference(
  reference: RawProjectRoleContinuityReference,
): ProjectRoleContinuityReference {
  switch (reference.reference_type) {
    case "object":
      return {
        referenceType: reference.reference_type,
        objectId: reference.object_id,
        label: reference.label,
      };
    case "assignment":
      return {
        referenceType: reference.reference_type,
        assignmentId: reference.assignment_id,
        label: reference.label,
      };
    case "commitment":
      return {
        referenceType: reference.reference_type,
        commitmentId: reference.commitment_id,
        label: reference.label,
      };
    case "nostr_event":
      return {
        referenceType: reference.reference_type,
        eventId: reference.event_id,
        label: reference.label,
      };
  }
}

function serializeReference(
  reference: ProjectRoleContinuityReference,
): Record<string, unknown> {
  switch (reference.referenceType) {
    case "object":
      return {
        reference_type: reference.referenceType,
        object_id: reference.objectId,
        label: reference.label,
      };
    case "assignment":
      return {
        reference_type: reference.referenceType,
        assignment_id: reference.assignmentId,
        label: reference.label,
      };
    case "commitment":
      return {
        reference_type: reference.referenceType,
        commitment_id: reference.commitmentId,
        label: reference.label,
      };
    case "nostr_event":
      return {
        reference_type: reference.referenceType,
        event_id: reference.eventId,
        label: reference.label,
      };
  }
}

export type ProjectRoleHistoryCursor = {
  projectRevision: number;
  entityType:
    | "role_assignment_proposal"
    | "role_assignment"
    | "role_checkpoint"
    | "role_handoff";
  entityId: string;
};

export type ProjectRoleHistoryItem =
  | { entityType: "proposal"; entity: ProjectRoleProposal }
  | { entityType: "assignment"; entity: ProjectRoleAssignment }
  | { entityType: "checkpoint"; entity: ProjectRoleCheckpoint }
  | { entityType: "handoff"; entity: ProjectRoleHandoff };

export type ProjectRoleHistoryPage = {
  projectRevision: number;
  projectionGeneration: number;
  items: ProjectRoleHistoryItem[];
  nextBefore?: ProjectRoleHistoryCursor;
};

type RawProjectRoleHistoryCursor = {
  project_revision: number;
  entity_type: ProjectRoleHistoryCursor["entityType"];
  entity_id: string;
};

type RawProjectRoleHistoryItem =
  | { entity_type: "proposal"; entity: RawProjectRoleProposal }
  | { entity_type: "assignment"; entity: RawProjectRoleAssignment }
  | { entity_type: "checkpoint"; entity: RawProjectRoleCheckpoint }
  | { entity_type: "handoff"; entity: RawProjectRoleHandoff };

export type RawProjectRoleHistoryPage = {
  project_revision: number;
  projection_generation: number;
  items: RawProjectRoleHistoryItem[];
  next_before?: RawProjectRoleHistoryCursor;
};

export async function getProjectViewRoleHistory(input: {
  projectRevision: number;
  projectionGeneration: number;
  roleId: string;
  limit?: number;
  before?: ProjectRoleHistoryCursor;
}): Promise<ProjectRoleHistoryPage> {
  const raw = await invokeTauri<RawProjectRoleHistoryPage>(
    "get_project_view_role_history",
    {
      input: {
        project_revision: input.projectRevision,
        projection_generation: input.projectionGeneration,
        role_id: input.roleId,
        limit: input.limit ?? 10,
        before: input.before
          ? {
              project_revision: input.before.projectRevision,
              entity_type: input.before.entityType,
              entity_id: input.before.entityId,
            }
          : undefined,
      },
    },
  );
  return {
    projectRevision: raw.project_revision,
    projectionGeneration: raw.projection_generation,
    items: raw.items.map(normalizeHistoryItem),
    nextBefore: raw.next_before
      ? {
          projectRevision: raw.next_before.project_revision,
          entityType: raw.next_before.entity_type,
          entityId: raw.next_before.entity_id,
        }
      : undefined,
  };
}

function normalizeHistoryItem(
  item: RawProjectRoleHistoryItem,
): ProjectRoleHistoryItem {
  switch (item.entity_type) {
    case "proposal":
      return {
        entityType: item.entity_type,
        entity: {
          proposalId: item.entity.proposal_id,
          roleId: item.entity.role_id,
          candidatePubkey: item.entity.candidate_pubkey,
          proposalType: item.entity.proposal_type,
          candidateAcceptedAt: item.entity.candidate_accepted_at,
          authorizedBy: item.entity.authorized_by,
          authorizedAt: item.entity.authorized_at,
          expectedTargetAssignmentId: item.entity.expected_target_assignment_id,
          expectedCandidateAssignmentId:
            item.entity.expected_candidate_assignment_id,
          expiresAt: item.entity.expires_at,
          status: item.entity.status,
          reason: item.entity.reason,
          createdBy: item.entity.created_by,
          createdAt: item.entity.created_at,
          resolvedAt: item.entity.resolved_at,
          entityRevision: item.entity.entity_revision,
          projectRevision: item.entity.project_revision,
        },
      };
    case "assignment":
      return {
        entityType: item.entity_type,
        entity: {
          assignmentId: item.entity.assignment_id,
          roleId: item.entity.role_id,
          memberPubkey: item.entity.member_pubkey,
          proposalId: item.entity.proposal_id,
          startedAt: item.entity.started_at,
          startedBy: item.entity.started_by,
          replacementRequestedAt: item.entity.replacement_requested_at,
          replacementRequestReason: item.entity.replacement_request_reason,
          unableReportedAt: item.entity.unable_reported_at,
          unableReportReason: item.entity.unable_report_reason,
          endedAt: item.entity.ended_at,
          endedBy: item.entity.ended_by,
          endedReason: item.entity.ended_reason,
          replacedByAssignmentId: item.entity.replaced_by_assignment_id,
          entityRevision: item.entity.entity_revision,
          projectRevision: item.entity.project_revision,
        },
      };
    case "checkpoint":
      return {
        entityType: item.entity_type,
        entity: normalizeCheckpoint(item.entity),
      };
    case "handoff":
      return {
        entityType: item.entity_type,
        entity: normalizeHandoff(item.entity),
      };
  }
}
import { invokeTauri } from "@/shared/api/tauri";
import type {
  ProjectRoleAssignment,
  ProjectRoleProposal,
  RawProjectRoleAssignment,
  RawProjectRoleProposal,
} from "@/shared/api/tauriProjectViewRole";
