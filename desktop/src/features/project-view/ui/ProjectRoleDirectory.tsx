import type { UserProfileLookup } from "@/features/profile/lib/identity";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import type { ProjectRoleDirectory as ProjectRoleDirectoryState } from "@/shared/api/tauriProjectViewRole";
import { Badge } from "@/shared/ui/badge";

export function ProjectRoleDirectory({
  actorProfiles,
  currentPubkey,
  directory,
}: {
  actorProfiles?: UserProfileLookup;
  currentPubkey?: string;
  directory: ProjectRoleDirectoryState;
}) {
  return (
    <div className="mt-3" data-testid="project-role-directory">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Collaboration roles
        </div>
        <span className="text-2xs text-muted-foreground">
          {directory.entries.length} of {directory.totalActiveRoles} active
        </span>
      </div>
      <ul className="mt-1.5 space-y-2">
        {directory.entries.map((entry) => (
          <li
            className="rounded-lg border border-border/70 px-2.5 py-2"
            key={entry.roleId}
          >
            <div className="flex flex-wrap items-center gap-1.5">
              <span className="text-xs font-medium">{entry.name}</span>
              <Badge variant={entry.level === "admin" ? "info" : "outline"}>
                {entry.level === "admin" ? "Leader" : "Role"}
              </Badge>
              {entry.isCurrentMemberRole ? (
                <Badge variant="success">Current</Badge>
              ) : null}
            </div>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
              {entry.purposeSummary}
            </p>
            <div className="mt-1.5 text-xs text-muted-foreground">
              {entry.assignment.status === "assigned" ? (
                <ProjectViewActor
                  compact
                  currentPubkey={currentPubkey}
                  profiles={actorProfiles}
                  pubkey={entry.assignment.memberPubkey}
                />
              ) : (
                <span>Vacant</span>
              )}
            </div>
          </li>
        ))}
      </ul>
      {directory.omittedActiveRoles > 0 ? (
        <p className="mt-1.5 text-xs text-muted-foreground">
          {directory.omittedActiveRoles} additional active{" "}
          {directory.omittedActiveRoles === 1 ? "Role is" : "Roles are"}{" "}
          available in the full directory.
        </p>
      ) : null}
    </div>
  );
}
