import { ProjectViewIntegrityError } from "@/shared/api/tauriProjectViewIntegrity";

type RawRoleBriefSource = {
  event_id: string;
  project_revision: number;
  item_revision: number;
  change_id: string;
  source_type: string;
};

export type RawRoleBriefRoleDirectory = {
  total_active_roles: number;
  entries: Array<{
    role_id: string;
    name: string;
    level: "admin" | "member";
    purpose_summary: string;
    assignment:
      | {
          status: "assigned";
          assignment_id: string;
          member_pubkey: string;
          source: RawRoleBriefSource;
        }
      | { status: "vacant" };
    is_current_member_role: boolean;
    role_source: RawRoleBriefSource;
  }>;
  omitted_active_roles: number;
};

export type ProjectRoleDirectoryEntry = {
  roleId: string;
  name: string;
  level: "admin" | "member";
  purposeSummary: string;
  assignment:
    | {
        status: "assigned";
        assignmentId: string;
        memberPubkey: string;
      }
    | { status: "vacant" };
  isCurrentMemberRole: boolean;
};

export type ProjectRoleDirectory = {
  totalActiveRoles: number;
  entries: ProjectRoleDirectoryEntry[];
  omittedActiveRoles: number;
};

type RoleBriefDirectoryMemberState =
  | { status: "candidate" }
  | { status: "assigned"; roleId: string };

export function normalizeRoleBriefRoleDirectory(
  raw: RawRoleBriefRoleDirectory | undefined,
  memberState: RoleBriefDirectoryMemberState,
): ProjectRoleDirectory {
  if (
    !raw ||
    !Array.isArray(raw.entries) ||
    !Number.isSafeInteger(raw.total_active_roles) ||
    raw.total_active_roles < 0 ||
    !Number.isSafeInteger(raw.omitted_active_roles) ||
    raw.omitted_active_roles < 0 ||
    raw.entries.length > raw.total_active_roles ||
    raw.omitted_active_roles !== raw.total_active_roles - raw.entries.length
  ) {
    throw new ProjectViewIntegrityError(
      "Role Brief contains invalid Role Directory counts",
    );
  }

  const roleIds = new Set<string>();
  let currentEntries = 0;
  const entries = raw.entries.map<ProjectRoleDirectoryEntry>((entry) => {
    if (
      roleIds.has(entry.role_id) ||
      entry.role_id.length === 0 ||
      entry.name.length === 0 ||
      entry.purpose_summary.length === 0 ||
      (entry.level !== "admin" && entry.level !== "member") ||
      typeof entry.is_current_member_role !== "boolean" ||
      (entry.assignment.status !== "assigned" &&
        entry.assignment.status !== "vacant")
    ) {
      throw new ProjectViewIntegrityError(
        "Role Brief contains an invalid Role Directory entry",
      );
    }
    roleIds.add(entry.role_id);
    if (entry.is_current_member_role) currentEntries += 1;

    const assignment: ProjectRoleDirectoryEntry["assignment"] =
      entry.assignment.status === "assigned"
        ? {
            status: "assigned",
            assignmentId: entry.assignment.assignment_id,
            memberPubkey: entry.assignment.member_pubkey,
          }
        : { status: "vacant" };
    if (
      assignment.status === "assigned" &&
      (assignment.assignmentId.length === 0 ||
        assignment.memberPubkey.length === 0)
    ) {
      throw new ProjectViewIntegrityError(
        "Role Brief contains an invalid assigned Role Directory entry",
      );
    }
    return {
      roleId: entry.role_id,
      name: entry.name,
      level: entry.level,
      purposeSummary: entry.purpose_summary,
      assignment,
      isCurrentMemberRole: entry.is_current_member_role,
    };
  });

  if (
    currentEntries > 1 ||
    (memberState.status === "candidate" && currentEntries !== 0) ||
    (memberState.status === "assigned" &&
      (currentEntries !== 1 ||
        !entries.some(
          (entry) =>
            entry.isCurrentMemberRole && entry.roleId === memberState.roleId,
        )))
  ) {
    throw new ProjectViewIntegrityError(
      "Role Brief Role Directory disagrees with its Member state",
    );
  }

  return {
    totalActiveRoles: raw.total_active_roles,
    entries,
    omittedActiveRoles: raw.omitted_active_roles,
  };
}

type CanonicalRole = {
  active: boolean;
  name: string;
  level: "admin" | "member";
};

type ActiveAssignment = {
  assignmentId: string;
  memberPubkey: string;
};

export function validateRoleBriefRoleDirectoryContinuity(
  directory: ProjectRoleDirectory,
  memberState: RoleBriefDirectoryMemberState,
  activeRoleCount: number,
  rolesById: ReadonlyMap<string, CanonicalRole>,
  activeAssignmentsByRoleId: ReadonlyMap<string, ActiveAssignment>,
) {
  if (directory.totalActiveRoles !== activeRoleCount) {
    throw new ProjectViewIntegrityError(
      "Role Brief Role Directory count disagrees with active Roles",
    );
  }
  for (const entry of directory.entries) {
    const role = rolesById.get(entry.roleId);
    const activeAssignment = activeAssignmentsByRoleId.get(entry.roleId);
    const shouldBeCurrent =
      memberState.status === "assigned" && memberState.roleId === entry.roleId;
    if (
      !role?.active ||
      role.name !== entry.name ||
      role.level !== entry.level ||
      entry.isCurrentMemberRole !== shouldBeCurrent ||
      (entry.assignment.status === "assigned"
        ? !activeAssignment ||
          activeAssignment.assignmentId !== entry.assignment.assignmentId ||
          activeAssignment.memberPubkey.toLowerCase() !==
            entry.assignment.memberPubkey.toLowerCase()
        : activeAssignment !== undefined)
    ) {
      throw new ProjectViewIntegrityError(
        "Role Brief Role Directory disagrees with Role continuity",
      );
    }
  }
}
