import * as React from "react";
import {
  AlertCircle,
  Boxes,
  CircleDot,
  Flag,
  GitBranch,
  Map as MapIcon,
  Plus,
  ShieldCheck,
} from "lucide-react";

import {
  countProjectViewFocus,
  indexProjectViewObjects,
  type ProjectViewCreateContext,
} from "@/features/project-view/model";
import { useProjectViewQuery } from "@/features/project-view/hooks";
import { ProjectViewDeleteDialog } from "@/features/project-view/ui/ProjectViewDeleteDialog";
import { ProjectViewInspector } from "@/features/project-view/ui/ProjectViewInspector";
import { ProjectViewMap } from "@/features/project-view/ui/ProjectViewMap";
import { ProjectViewObjectDialog } from "@/features/project-view/ui/ProjectViewObjectDialog";
import { ProjectViewObjectCard } from "@/features/project-view/ui/ProjectViewObjectCard";
import { ProjectViewInitialize } from "@/features/project-view/ui/ProjectViewInitialize";
import {
  ProjectViewErrorState,
  ProjectViewForbiddenState,
  ProjectViewLoadingState,
  ProjectViewUnsupportedState,
} from "@/features/project-view/ui/ProjectViewStates";
import type {
  ProjectView,
  ProjectViewLoadResult,
  ProjectViewObject,
  ProjectViewObjectType,
} from "@/shared/api/tauriProjectView";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

type ProjectViewScreenProps = {
  onSelectObject: (objectId: string | undefined) => void;
  selectedObjectId?: string;
};

function FocusMetric({
  icon,
  label,
  value,
}: {
  icon: React.ReactNode;
  label: string;
  value: number;
}) {
  return (
    <div className="rounded-xl border border-border/70 bg-card/60 p-3">
      <div className="flex items-center gap-2 text-muted-foreground">
        {icon}
        <span className="text-2xs font-semibold uppercase tracking-wider">
          {label}
        </span>
      </div>
      <div className="mt-1 text-xl font-semibold tabular-nums">{value}</div>
    </div>
  );
}

function ProjectProfile({
  onSelectObject,
  selectedObjectId,
  view,
}: {
  onSelectObject: (objectId: string) => void;
  selectedObjectId?: string;
  view: ProjectView;
}) {
  const profile = view.profile;
  const relatedIssueCount =
    view.issueReferencesByTarget[profile.id]?.length ?? 0;
  return (
    <section
      className="overflow-hidden rounded-2xl border border-border/70 bg-card/60 shadow-xs"
      data-testid="project-view-profile"
    >
      <button
        className="w-full p-5 text-left transition-colors hover:bg-muted/20 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        onClick={() => onSelectObject(profile.id)}
        type="button"
      >
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline">Project profile</Badge>
          {selectedObjectId === profile.id ? (
            <Badge variant="info">Inspecting</Badge>
          ) : null}
          {relatedIssueCount > 0 ? (
            <Badge variant="warning">
              <AlertCircle className="mr-1 h-3 w-3" />
              {relatedIssueCount} related{" "}
              {relatedIssueCount === 1 ? "issue" : "issues"}
            </Badge>
          ) : null}
        </div>
        <h1 className="mt-3 text-2xl font-semibold tracking-tight">
          {profile.data.name}
        </h1>
        <p className="mt-2 max-w-4xl text-sm leading-relaxed text-muted-foreground">
          {profile.data.positioning}
        </p>
      </button>
      <div className="grid border-t border-border/70 sm:grid-cols-3">
        {[
          ["Purpose", profile.data.purpose],
          ["Problem", profile.data.problem],
          ["Scope", profile.data.scope],
        ].map(([label, value], index) => (
          <div
            className={
              index === 0
                ? "p-4"
                : "border-t border-border/70 p-4 sm:border-l sm:border-t-0"
            }
            key={label}
          >
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              {label}
            </div>
            <p className="mt-1.5 text-sm leading-relaxed">{value}</p>
          </div>
        ))}
      </div>
    </section>
  );
}

function SupportingObjects({
  onCreateObject,
  onSelectObject,
  selectedObjectId,
  view,
}: {
  onCreateObject: (
    objectType: "role" | "resource",
    context?: ProjectViewCreateContext,
  ) => void;
  onSelectObject: (objectId: string) => void;
  selectedObjectId?: string;
  view: ProjectView;
}) {
  return (
    <section className="grid gap-5 xl:grid-cols-2">
      <div>
        <div className="mb-2 flex items-center gap-2">
          <Flag className="h-4 w-4 text-muted-foreground" />
          <h2 className="text-sm font-semibold">Roles</h2>
          <span className="text-xs text-muted-foreground">
            Semantic responsibilities
          </span>
          <Button
            className="ml-auto h-7"
            onClick={() => onCreateObject("role")}
            size="sm"
            type="button"
            variant="ghost"
          >
            <Plus />
            Add Role
          </Button>
        </div>
        {view.roles.length > 0 ? (
          <div className="grid gap-2 sm:grid-cols-2">
            {view.roles.map((role) => (
              <ProjectViewObjectCard
                key={role.id}
                object={role}
                onSelect={onSelectObject}
                selected={selectedObjectId === role.id}
                size="compact"
              />
            ))}
          </div>
        ) : (
          <div className="rounded-xl border border-dashed border-border/70 p-4 text-xs text-muted-foreground">
            No semantic roles declared.
          </div>
        )}
      </div>
      <div>
        <div className="mb-2 flex items-center gap-2">
          <Boxes className="h-4 w-4 text-muted-foreground" />
          <h2 className="text-sm font-semibold">Resources</h2>
          <span className="text-xs text-muted-foreground">
            Stable project entry points
          </span>
          <Button
            className="ml-auto h-7"
            onClick={() => onCreateObject("resource")}
            size="sm"
            type="button"
            variant="ghost"
          >
            <Plus />
            Add Resource
          </Button>
        </div>
        {view.resources.length > 0 ? (
          <div className="grid gap-2 sm:grid-cols-2">
            {view.resources.map((resource) => (
              <ProjectViewObjectCard
                key={resource.id}
                object={resource}
                onSelect={onSelectObject}
                selected={selectedObjectId === resource.id}
                size="compact"
              />
            ))}
          </div>
        ) : (
          <div className="rounded-xl border border-dashed border-border/70 p-4 text-xs text-muted-foreground">
            No resources declared.
          </div>
        )}
      </div>
    </section>
  );
}

function ReadyProjectView({
  activeObjectCount,
  onSelectObject,
  projectRevision,
  projectionGeneration,
  relayPubkey,
  onRefresh,
  selectedObjectId,
  updatedAt,
  view,
}: Extract<ProjectViewLoadResult, { status: "ready" }> &
  ProjectViewScreenProps & { onRefresh: () => void }) {
  type EditorRequest =
    | {
        mode: "create";
        initialType?: Exclude<ProjectViewObjectType, "project_profile">;
        context?: ProjectViewCreateContext;
      }
    | { mode: "edit"; object: ProjectViewObject };

  const [editor, setEditor] = React.useState<EditorRequest>();
  const [deleteTarget, setDeleteTarget] = React.useState<ProjectViewObject>();
  const objectsById = React.useMemo(
    () => indexProjectViewObjects(view),
    [view],
  );
  const selectedObject = selectedObjectId
    ? objectsById.get(selectedObjectId)
    : undefined;
  const focus = React.useMemo(() => countProjectViewFocus(view), [view]);
  const selectObject = React.useCallback(
    (objectId: string) => onSelectObject(objectId),
    [onSelectObject],
  );
  const createObject = React.useCallback(
    (
      objectType: Exclude<ProjectViewObjectType, "project_profile">,
      context?: ProjectViewCreateContext,
    ) => setEditor({ mode: "create", initialType: objectType, context }),
    [],
  );

  return (
    <div className="relative flex min-h-0 flex-1 overflow-hidden">
      <main className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-7xl space-y-6 p-5 pb-12">
          <div className="flex justify-end">
            <Button
              data-testid="project-view-add"
              onClick={() => setEditor({ mode: "create" })}
              type="button"
            >
              <Plus />
              Add
            </Button>
          </div>
          <ProjectProfile
            onSelectObject={selectObject}
            selectedObjectId={selectedObjectId}
            view={view}
          />

          <section>
            <div className="mb-2 flex items-center gap-2">
              <CircleDot className="h-4 w-4 text-muted-foreground" />
              <h2 className="text-sm font-semibold">Current focus</h2>
              <span className="text-xs text-muted-foreground">
                Derived from explicit object states
              </span>
            </div>
            <div className="grid grid-cols-2 gap-2 lg:grid-cols-4">
              <FocusMetric
                icon={<GitBranch className="h-3.5 w-3.5" />}
                label="Active plans"
                value={focus.activePlans}
              />
              <FocusMetric
                icon={<MapIcon className="h-3.5 w-3.5" />}
                label="Active stages"
                value={focus.activeStages}
              />
              <FocusMetric
                icon={<CircleDot className="h-3.5 w-3.5" />}
                label="Open issues"
                value={focus.openIssues}
              />
              <FocusMetric
                icon={<ShieldCheck className="h-3.5 w-3.5" />}
                label="Work in progress"
                value={focus.inProgressWork}
              />
            </div>
          </section>

          <ProjectViewMap
            onCreateObject={createObject}
            onSelectObject={selectObject}
            selectedObjectId={selectedObjectId}
            view={view}
          />
          <SupportingObjects
            onCreateObject={createObject}
            onSelectObject={selectObject}
            selectedObjectId={selectedObjectId}
            view={view}
          />

          <footer className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border/70 pt-4 text-2xs text-muted-foreground">
            <span>{activeObjectCount} verified objects</span>
            <span>Project revision {projectRevision}</span>
            <span>Projection generation {projectionGeneration}</span>
            <span>Updated {new Date(updatedAt).toLocaleString()}</span>
            <span className="sr-only">Relay signer {relayPubkey}</span>
          </footer>
        </div>
      </main>
      {selectedObject ? (
        <ProjectViewInspector
          object={selectedObject}
          objectsById={objectsById}
          onClose={() => onSelectObject(undefined)}
          onDelete={setDeleteTarget}
          onEdit={(object) => setEditor({ mode: "edit", object })}
          onSelectObject={selectObject}
          view={view}
        />
      ) : null}
      {editor ? (
        <ProjectViewObjectDialog
          context={editor.mode === "create" ? editor.context : undefined}
          initialType={
            editor.mode === "create" ? editor.initialType : undefined
          }
          mode={editor.mode}
          object={editor.mode === "edit" ? editor.object : undefined}
          onApplied={(objectId) => {
            if (objectId) onSelectObject(objectId);
          }}
          onOpenChange={(open) => {
            if (!open) setEditor(undefined);
          }}
          onReviewLatest={onRefresh}
          open
          projectRevision={projectRevision}
          view={view}
        />
      ) : null}
      <ProjectViewDeleteDialog
        object={deleteTarget}
        onDeleted={() => onSelectObject(undefined)}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(undefined);
        }}
        onReviewLatest={onRefresh}
        open={Boolean(deleteTarget)}
        projectRevision={projectRevision}
        view={view}
      />
    </div>
  );
}

export function ProjectViewScreen({
  onSelectObject,
  selectedObjectId,
}: ProjectViewScreenProps) {
  const query = useProjectViewQuery();

  return (
    <div className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
      <TopChromeInsetHeader flush>
        <header
          className="flex h-12 items-center gap-3 px-5"
          data-tauri-drag-region
        >
          <MapIcon className="h-4 w-4 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-semibold">View</div>
            <div className="text-2xs text-muted-foreground">
              Community project context
            </div>
          </div>
          {query.data?.status === "ready" ||
          query.data?.status === "uninitialized" ? (
            <Badge variant="outline">Editable</Badge>
          ) : null}
          {query.data?.status === "ready" ? (
            <Badge variant="success">
              <ShieldCheck className="mr-1 h-3 w-3" />
              Verified
            </Badge>
          ) : null}
        </header>
      </TopChromeInsetHeader>

      {query.isPending ? <ProjectViewLoadingState /> : null}
      {query.isError ? (
        <ProjectViewErrorState
          message={
            query.error instanceof Error
              ? query.error.message
              : "The Relay returned an unexpected Project View response."
          }
          onRetry={() => void query.refetch()}
          retrying={query.isFetching}
        />
      ) : null}
      {query.data?.status === "unsupported" ? (
        <ProjectViewUnsupportedState />
      ) : null}
      {query.data?.status === "forbidden" ? (
        <ProjectViewForbiddenState />
      ) : null}
      {query.data?.status === "uninitialized" ? (
        <ProjectViewInitialize onReviewLatest={() => void query.refetch()} />
      ) : null}
      {query.data?.status === "ready" ? (
        <ReadyProjectView
          {...query.data}
          onRefresh={() => void query.refetch()}
          onSelectObject={onSelectObject}
          selectedObjectId={selectedObjectId}
        />
      ) : null}
    </div>
  );
}
