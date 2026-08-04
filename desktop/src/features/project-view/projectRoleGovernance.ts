import type {
  ProjectRoleLevel,
  ProjectViewRoleContinuity,
} from "@/shared/api/tauriProjectView";

export type ProjectRoleGovernanceCapabilities = {
  isOwner: boolean;
  isActiveLeader: boolean;
  actingAssignmentId?: string;
  canCreateMemberRole: boolean;
  canCreateAdminRole: boolean;
};

/**
 * Derive UI capabilities from one verified membership/Role snapshot.
 *
 * Relay authorization remains authoritative. This helper intentionally fails
 * closed unless a non-owner has both Community admin membership and an active
 * Assignment to an active admin Role.
 */
export function projectRoleGovernanceCapabilities(
  continuity: ProjectViewRoleContinuity | undefined,
  currentPubkey: string | undefined,
): ProjectRoleGovernanceCapabilities {
  const actor = currentPubkey?.trim().toLowerCase();
  if (!continuity || !actor) {
    return {
      isOwner: false,
      isActiveLeader: false,
      canCreateMemberRole: false,
      canCreateAdminRole: false,
    };
  }

  const membership = continuity.members.find(
    (member) => member.pubkey.toLowerCase() === actor,
  );
  const isOwner = membership?.role === "owner";
  const leaderAssignment = continuity.assignments.find((assignment) => {
    if (assignment.endedAt || assignment.memberPubkey.toLowerCase() !== actor) {
      return false;
    }
    const role = continuity.roles.find(
      (candidate) => candidate.roleId === assignment.roleId,
    );
    return role?.active === true && role.level === "admin";
  });
  const isActiveLeader = Boolean(
    membership?.role === "admin" && leaderAssignment,
  );

  return {
    isOwner,
    isActiveLeader,
    actingAssignmentId:
      !isOwner && isActiveLeader ? leaderAssignment?.assignmentId : undefined,
    canCreateMemberRole: isOwner || isActiveLeader,
    canCreateAdminRole: isOwner,
  };
}

export function canGovernProjectRole(
  capabilities: ProjectRoleGovernanceCapabilities,
  level: ProjectRoleLevel,
): boolean {
  return (
    capabilities.isOwner || (capabilities.isActiveLeader && level === "member")
  );
}
