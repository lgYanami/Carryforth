import { Crown, UserRound, UserRoundX } from "lucide-react";

import type { UserProfileLookup } from "@/features/profile/lib/identity";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import type {
  ProjectRoleAssignment,
  ProjectRoleDefinition,
  ProjectViewObjectOf,
} from "@/shared/api/tauriProjectView";
import { cn } from "@/shared/lib/cn";
import { Badge } from "@/shared/ui/badge";

export function ProjectRoleCard({
  actorProfiles,
  currentAssignment,
  currentPubkey,
  definition,
  object,
  onSelect,
  selected = false,
}: {
  actorProfiles?: UserProfileLookup;
  currentAssignment?: ProjectRoleAssignment;
  currentPubkey?: string;
  definition: ProjectRoleDefinition;
  object: ProjectViewObjectOf<"role">;
  onSelect: (objectId: string) => void;
  selected?: boolean;
}) {
  const leader = definition.level === "admin";
  return (
    <button
      aria-label={`Inspect Role ${object.data.name}`}
      className={cn(
        "group w-full rounded-xl border bg-card/70 p-3 text-left shadow-xs transition-colors hover:border-primary/40 hover:bg-card focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring",
        selected
          ? "border-primary/60 ring-1 ring-primary/30"
          : "border-border/70",
      )}
      data-object-id={object.id}
      data-testid={`project-role-card-${object.id}`}
      onClick={() => onSelect(object.id)}
      type="button"
    >
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <Badge variant={leader ? "info" : "outline"}>
              {leader ? (
                <Crown className="mr-1 h-3 w-3" />
              ) : (
                <UserRound className="mr-1 h-3 w-3" />
              )}
              {leader ? "Leader" : "Role"}
            </Badge>
            {!definition.active ? (
              <Badge variant="secondary">Inactive</Badge>
            ) : currentAssignment ? (
              <Badge variant="success">Assigned</Badge>
            ) : (
              <Badge variant="warning">Vacant</Badge>
            )}
          </div>
          <h3 className="mt-2 text-sm font-semibold leading-snug">
            {object.data.name}
          </h3>
          <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-muted-foreground">
            {object.data.purpose}
          </p>
        </div>
      </div>
      <div className="mt-3 flex min-w-0 items-center gap-2 border-t border-border/60 pt-2 text-xs">
        {currentAssignment ? (
          <>
            <span className="shrink-0 text-muted-foreground">Held by</span>
            <ProjectViewActor
              compact
              currentPubkey={currentPubkey}
              profiles={actorProfiles}
              pubkey={currentAssignment.memberPubkey}
            />
          </>
        ) : (
          <>
            <UserRoundX className="h-3.5 w-3.5 text-amber-600 dark:text-amber-400" />
            <span className="font-medium text-amber-700 dark:text-amber-300">
              No current assignee
            </span>
          </>
        )}
      </div>
    </button>
  );
}
