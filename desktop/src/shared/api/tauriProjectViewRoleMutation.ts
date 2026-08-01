import { invokeTauri } from "@/shared/api/tauri";
import {
  serializeCheckpointContent,
  serializeHandoffContent,
  type ProjectRoleCheckpointContent,
  type ProjectRoleHandoffContent,
} from "@/shared/api/tauriProjectViewRoleHistory";

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
    })
  | (ProjectViewRoleMutationBase & {
      operation: "append_checkpoint";
      basedOnProjectRevision: number;
      content: ProjectRoleCheckpointContent;
      supersedesCheckpointId?: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "append_handoff";
      toAssignmentId?: string;
      checkpointId?: string;
      content: ProjectRoleHandoffContent;
      cause: "planned" | "other";
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
      checkpoint_id?: string;
      handoff_id?: string;
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
      checkpointId?: string;
      handoffId?: string;
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
    case "append_checkpoint":
      return {
        ...common,
        based_on_project_revision: intent.basedOnProjectRevision,
        content: serializeCheckpointContent(intent.content),
        supersedes_checkpoint_id: intent.supersedesCheckpointId,
      };
    case "append_handoff":
      return {
        ...common,
        to_assignment_id: intent.toAssignmentId,
        checkpoint_id: intent.checkpointId,
        content: serializeHandoffContent(intent.content),
        cause: intent.cause,
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
    checkpointId: raw.checkpoint_id,
    handoffId: raw.handoff_id,
    changedEntities: raw.changed_entities,
  };
}
