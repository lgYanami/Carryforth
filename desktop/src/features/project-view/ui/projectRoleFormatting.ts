import type { ProjectRoleAssignment } from "@/shared/api/tauriProjectView";

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

export function formatProjectRoleDateTime(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : dateTimeFormatter.format(date);
}

export function findActiveProjectRoleAssignment(
  assignments: ProjectRoleAssignment[],
  roleId: string,
) {
  return assignments.find(
    (assignment) => assignment.roleId === roleId && !assignment.endedAt,
  );
}
