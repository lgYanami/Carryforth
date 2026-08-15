import type {
  ProjectRoleDefinition,
  ProjectViewRoleContinuity,
} from "@/shared/api/tauriProjectView";

export type ProjectViewRoleLifecycleState = {
  blocked: boolean;
  hasActiveAssignment: boolean;
  hasOpenProposal: boolean;
  hasResponsibleWork: boolean;
  message?: string;
};

/** Derive every Role lifecycle fence shared by edit and delete surfaces. */
export function projectViewRoleLifecycleState(
  definition: ProjectRoleDefinition | undefined,
  continuity: ProjectViewRoleContinuity | undefined,
): ProjectViewRoleLifecycleState {
  const hasActiveAssignment = Boolean(
    definition &&
      continuity?.assignments.some(
        (assignment) =>
          assignment.roleId === definition.roleId && !assignment.endedAt,
      ),
  );
  const hasOpenProposal = Boolean(
    definition &&
      continuity?.proposals.some(
        (proposal) =>
          proposal.roleId === definition.roleId && proposal.status === "open",
      ),
  );
  const hasResponsibleWork = Boolean(
    definition &&
      continuity?.workResponsibilities.some(
        (responsibility) => responsibility.roleId === definition.roleId,
      ),
  );
  return {
    blocked: hasActiveAssignment || hasOpenProposal || hasResponsibleWork,
    hasActiveAssignment,
    hasOpenProposal,
    hasResponsibleWork,
    message: hasActiveAssignment
      ? "This Role has an active Assignment. End or replace the tenure before deactivating or deleting the Role."
      : hasOpenProposal
        ? "This Role has an open Proposal. Resolve or withdraw it before deactivating or deleting the Role."
        : hasResponsibleWork
          ? "This Role is responsible for Work. Clear or reassign that responsibility before deactivating or deleting the Role."
          : undefined,
  };
}
