import { Link2, Network, Pencil, Trash2 } from "lucide-react";

import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { ProjectViewCreateContext } from "@/features/project-view/model";
import type { ProjectViewRoleLifecycleState } from "@/features/project-view/projectViewRoleLifecycle";
import { ProjectRoleInspector } from "@/features/project-view/ui/ProjectRoleInspector";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import { ProjectViewCreateMenu } from "@/features/project-view/ui/ProjectViewCreateMenu";
import { ProjectViewDetail } from "@/features/project-view/ui/ProjectViewObjectDetails";
import { ProjectWorkContinuity } from "@/features/project-view/ui/ProjectWorkContinuity";
import type {
  ProjectRoleDefinition,
  ProjectViewObject,
  ProjectViewObjectType,
  ProjectViewRoleContinuity,
} from "@/shared/api/tauriProjectView";
import { Button } from "@/shared/ui/button";

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function formatDateTime(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : dateTimeFormatter.format(date);
}

/** Header actions for reading and maintaining only the current object. */
export function ProjectViewObjectActions({
  canMaintain,
  canCreateRole,
  lifecycle,
  object,
  onCreate,
  onDelete,
  onEdit,
  onManageContext,
  onShowInProjectContext,
}: {
  canMaintain: boolean;
  canCreateRole: boolean;
  lifecycle: ProjectViewRoleLifecycleState;
  object: ProjectViewObject;
  onCreate: (
    initialType?: Exclude<ProjectViewObjectType, "project_profile">,
    context?: ProjectViewCreateContext,
  ) => void;
  onDelete: (object: ProjectViewObject) => void;
  onEdit: (object: ProjectViewObject) => void;
  onManageContext: (object: ProjectViewObject) => void;
  onShowInProjectContext?: (object: ProjectViewObject) => void;
}) {
  return (
    <fieldset className="flex min-w-0 flex-wrap gap-2 border-0 p-0">
      <legend className="sr-only">Current object actions</legend>
      <ProjectViewCreateMenu
        canCreateRole={canCreateRole}
        object={object}
        onCreate={onCreate}
      />
      {canMaintain ? (
        <>
          <Button
            data-testid="project-view-edit-current"
            onClick={() => onEdit(object)}
            size="sm"
            type="button"
            variant="outline"
          >
            <Pencil />
            Edit
          </Button>
          <Button
            data-testid="project-view-delete-current"
            disabled={lifecycle.blocked}
            onClick={() => onDelete(object)}
            size="sm"
            title={lifecycle.message}
            type="button"
            variant="outline"
          >
            <Trash2 />
            Delete
          </Button>
          <Button
            data-testid="project-view-manage-context"
            onClick={() => onManageContext(object)}
            size="sm"
            type="button"
            variant="outline"
          >
            <Link2 />
            Manage Context
          </Button>
        </>
      ) : null}
      {onShowInProjectContext ? (
        <Button
          data-testid="project-view-show-in-project-context"
          onClick={() => onShowInProjectContext(object)}
          size="sm"
          type="button"
          variant="outline"
        >
          <Network />
          Show in Project Context
        </Button>
      ) : null}
    </fieldset>
  );
}

/** Continuity, lifecycle, and verified provenance for the current object. */
export function ProjectViewObjectMaintenance({
  actorProfiles,
  currentPubkey,
  lifecycle,
  object,
  projectionGeneration,
  projectRevision,
  roleContinuity,
  roleDefinition,
}: {
  actorProfiles?: UserProfileLookup;
  currentPubkey?: string;
  lifecycle: ProjectViewRoleLifecycleState;
  object: ProjectViewObject;
  projectionGeneration: number;
  projectRevision: number;
  roleContinuity?: ProjectViewRoleContinuity;
  roleDefinition?: ProjectRoleDefinition;
}) {
  return (
    <section className="space-y-6" data-testid="project-view-maintenance">
      {lifecycle.message ? (
        <p
          className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs leading-relaxed text-muted-foreground"
          data-testid="project-role-lifecycle-guard"
        >
          {lifecycle.message}
        </p>
      ) : null}

      {object.objectType === "role" && roleContinuity && roleDefinition ? (
        <ProjectRoleInspector
          actorProfiles={actorProfiles}
          continuity={roleContinuity}
          currentPubkey={currentPubkey}
          definition={roleDefinition}
          projectionGeneration={projectionGeneration}
          projectRevision={projectRevision}
        />
      ) : null}

      {object.objectType === "work" && roleContinuity ? (
        <ProjectWorkContinuity
          continuity={roleContinuity}
          currentPubkey={currentPubkey}
          projectRevision={projectRevision}
          workId={object.id}
        />
      ) : null}

      <section
        className="space-y-4 rounded-xl border border-border/70 bg-card/50 p-5"
        data-testid="project-view-provenance"
      >
        <h2 className="text-sm font-semibold">Verified projection</h2>
        <div className="grid gap-4 sm:grid-cols-3">
          <ProjectViewDetail label="Object revision">
            {object.objectRevision}
          </ProjectViewDetail>
          <ProjectViewDetail label="Project revision">
            {object.projectRevision}
          </ProjectViewDetail>
          <ProjectViewDetail label="Projection generation">
            {projectionGeneration}
          </ProjectViewDetail>
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          <ProjectViewDetail label="Created">
            <span>{formatDateTime(object.createdAt)}</span>
            <span className="mt-1 block text-xs text-muted-foreground">
              by{" "}
              <ProjectViewActor
                currentPubkey={currentPubkey}
                profiles={actorProfiles}
                pubkey={object.createdBy}
                pubkeyTestId="project-view-created-by"
              />
            </span>
          </ProjectViewDetail>
          <ProjectViewDetail label="Last updated">
            <span>{formatDateTime(object.updatedAt)}</span>
            <span className="mt-1 block text-xs text-muted-foreground">
              by{" "}
              <ProjectViewActor
                currentPubkey={currentPubkey}
                profiles={actorProfiles}
                pubkey={object.updatedBy}
                pubkeyTestId="project-view-updated-by"
              />
            </span>
          </ProjectViewDetail>
        </div>
        <ProjectViewDetail label="Object ID">
          <span className="break-all font-mono text-xs">{object.id}</span>
        </ProjectViewDetail>
      </section>
    </section>
  );
}
