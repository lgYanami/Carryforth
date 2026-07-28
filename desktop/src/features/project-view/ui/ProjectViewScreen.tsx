import * as React from "react";
import {
  AlertCircle,
  Boxes,
  CircleDot,
  Flag,
  GitBranch,
  Map as MapIcon,
  Plus,
  RefreshCw,
  ShieldCheck,
  WifiOff,
} from "lucide-react";

import {
  useManagedAgentsQuery,
  useRelayAgentsQuery,
} from "@/features/agents/hooks";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import {
  countProjectViewFocus,
  indexProjectViewObjects,
  type ProjectViewCreateContext,
} from "@/features/project-view/model";
import {
  useProjectViewLiveSync,
  useProjectViewQuery,
} from "@/features/project-view/hooks";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import { ProjectViewDeleteDialog } from "@/features/project-view/ui/ProjectViewDeleteDialog";
import { ProjectViewInspector } from "@/features/project-view/ui/ProjectViewInspector";
import { ProjectViewMap } from "@/features/project-view/ui/ProjectViewMap";
import { ProjectViewObjectDialog } from "@/features/project-view/ui/ProjectViewObjectDialog";
import { ProjectViewObjectCard } from "@/features/project-view/ui/ProjectViewObjectCard";
import {
  createProjectViewInitializationDraft,
  isProjectViewInitializationDraftDirty,
  ProjectViewInitialize,
  type ProjectViewInitializationDraft,
} from "@/features/project-view/ui/ProjectViewInitialize";
import {
  ProjectViewErrorState,
  ProjectViewForbiddenState,
  ProjectViewLoadingState,
  ProjectViewUnsupportedState,
} from "@/features/project-view/ui/ProjectViewStates";
import type {
  ProjectView,
  ProjectViewLoadResult,
  ProjectViewMutationResult,
  ProjectViewObject,
  ProjectViewObjectType,
} from "@/shared/api/tauriProjectView";
import { useIdentityQuery } from "@/shared/api/hooks";
import {
  isRelayConnectionDegraded,
  useRelayConnection,
} from "@/shared/api/useRelayConnection";
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
  actorProfiles,
  currentPubkey,
  onSelectObject,
  selectedObjectId,
  view,
}: {
  actorProfiles?: UserProfileLookup;
  currentPubkey?: string;
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
        <div className="mt-3 flex items-center gap-1 text-2xs text-muted-foreground">
          <span>Updated {new Date(profile.updatedAt).toLocaleString()} by</span>
          <ProjectViewActor
            compact
            currentPubkey={currentPubkey}
            profiles={actorProfiles}
            pubkey={profile.updatedBy}
          />
        </div>
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
  actorProfiles,
  currentPubkey,
  onCreateObject,
  onSelectObject,
  selectedObjectId,
  view,
}: {
  actorProfiles?: UserProfileLookup;
  currentPubkey?: string;
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
                actorProfiles={actorProfiles}
                currentPubkey={currentPubkey}
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
                actorProfiles={actorProfiles}
                currentPubkey={currentPubkey}
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

function InitializationDraftRecovery({
  conflict,
  draft,
  onDiscard,
  projectRevision,
}: {
  conflict?: Extract<ProjectViewMutationResult, { status: "conflict" }>;
  draft: ProjectViewInitializationDraft;
  onDiscard: () => void;
  projectRevision: number;
}) {
  return (
    <section
      className="rounded-xl border border-amber-500/40 bg-amber-500/10 p-4"
      data-testid="project-view-initialization-draft"
      role="status"
    >
      <div className="flex flex-wrap items-start gap-3">
        <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold">
            Initialization draft preserved
          </h2>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
            {conflict
              ? `Your atomic initialization was based on revision ${conflict.expectedProjectRevision}; this verified View is revision ${projectRevision}. Nothing from the draft was written.`
              : `This View became initialized at revision ${projectRevision} while you were drafting. The draft was not submitted or merged.`}
            {
              " Review it below, then apply anything still relevant through normal object edits."
            }
          </p>
          <details className="mt-3 rounded-lg border border-border/70 bg-background/70 p-3">
            <summary className="cursor-pointer text-xs font-semibold">
              Review preserved draft
            </summary>
            <div className="mt-3 space-y-3 text-xs text-muted-foreground">
              <div>
                <span className="font-semibold text-foreground">
                  {draft.profile.name || "Untitled project"}
                </span>
                {draft.profile.positioning ? (
                  <p className="mt-1 whitespace-pre-wrap">
                    {draft.profile.positioning}
                  </p>
                ) : null}
              </div>
              <ul className="space-y-2">
                {draft.goals.map((goal) => (
                  <li key={goal.key}>
                    <span className="font-semibold text-foreground">
                      {goal.title || "Untitled goal"}
                    </span>
                    {goal.desiredOutcome ? (
                      <p className="mt-0.5 whitespace-pre-wrap">
                        {goal.desiredOutcome}
                      </p>
                    ) : null}
                  </li>
                ))}
              </ul>
            </div>
          </details>
        </div>
        <Button onClick={onDiscard} size="sm" type="button" variant="outline">
          Discard preserved draft
        </Button>
      </div>
    </section>
  );
}

function ReadyProjectView({
  activeObjectCount,
  initializationConflict,
  initializationDraft,
  onDiscardInitializationDraft,
  onSelectObject,
  projectRevision,
  projectionGeneration,
  relayPubkey,
  onRefresh,
  selectedObjectId,
  syncMessage,
  syncState,
  updatedAt,
  view,
}: Extract<ProjectViewLoadResult, { status: "ready" }> &
  ProjectViewScreenProps & {
    initializationConflict?: Extract<
      ProjectViewMutationResult,
      { status: "conflict" }
    >;
    initializationDraft?: ProjectViewInitializationDraft;
    onDiscardInitializationDraft: () => void;
    onRefresh: () => Promise<unknown>;
    syncMessage?: string;
    syncState?: "refreshing" | "stale";
  }) {
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
  const actorPubkeys = React.useMemo(() => {
    const pubkeys = new Set<string>();
    for (const object of objectsById.values()) {
      pubkeys.add(object.createdBy);
      pubkeys.add(object.updatedBy);
    }
    return [...pubkeys];
  }, [objectsById]);
  const actorProfilesQuery = useUsersBatchQuery(actorPubkeys);
  const managedAgentsQuery = useManagedAgentsQuery();
  const relayAgentsQuery = useRelayAgentsQuery();
  const identityQuery = useIdentityQuery();
  const actorProfiles = React.useMemo<UserProfileLookup>(() => {
    const profiles = { ...(actorProfilesQuery.data?.profiles ?? {}) };
    const actorSet = new Set(
      actorPubkeys.map((pubkey) => pubkey.toLowerCase()),
    );
    const knownAgents = [
      ...(relayAgentsQuery.data ?? []),
      ...(managedAgentsQuery.data ?? []),
    ];
    for (const agent of knownAgents) {
      const pubkey = agent.pubkey.toLowerCase();
      if (!actorSet.has(pubkey)) continue;
      const existing = profiles[pubkey];
      profiles[pubkey] = {
        avatarUrl:
          existing?.avatarUrl ??
          ("avatarUrl" in agent ? agent.avatarUrl : null),
        displayName: existing?.displayName ?? agent.name,
        isAgent: true,
        name: existing?.name ?? agent.name,
        nip05Handle: existing?.nip05Handle ?? null,
        ownerPubkey: existing?.ownerPubkey ?? null,
      };
    }
    return profiles;
  }, [
    actorProfilesQuery.data?.profiles,
    actorPubkeys,
    managedAgentsQuery.data,
    relayAgentsQuery.data,
  ]);
  const currentPubkey = identityQuery.data?.pubkey;
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
          {initializationDraft ? (
            <InitializationDraftRecovery
              conflict={initializationConflict}
              draft={initializationDraft}
              onDiscard={onDiscardInitializationDraft}
              projectRevision={projectRevision}
            />
          ) : null}
          <div className="flex items-start gap-3">
            {syncMessage ? (
              <div
                className={
                  syncState === "stale"
                    ? "flex min-w-0 flex-1 items-start gap-2 rounded-xl border border-amber-500/40 bg-amber-500/10 px-3 py-2.5"
                    : "flex min-w-0 flex-1 items-start gap-2 rounded-xl border border-border/70 bg-muted/30 px-3 py-2.5"
                }
                data-testid="project-view-sync-state"
                role="status"
              >
                {syncState === "stale" ? (
                  <WifiOff className="mt-0.5 h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
                ) : (
                  <RefreshCw className="mt-0.5 h-4 w-4 shrink-0 animate-spin text-muted-foreground" />
                )}
                <p className="text-xs leading-relaxed text-muted-foreground">
                  {syncMessage}
                </p>
              </div>
            ) : (
              <div className="flex-1" />
            )}
            <Button
              className="shrink-0"
              data-testid="project-view-add"
              onClick={() => setEditor({ mode: "create" })}
              type="button"
            >
              <Plus />
              Add
            </Button>
          </div>
          <ProjectProfile
            actorProfiles={actorProfiles}
            currentPubkey={currentPubkey}
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
            actorProfiles={actorProfiles}
            currentPubkey={currentPubkey}
            onCreateObject={createObject}
            onSelectObject={selectObject}
            selectedObjectId={selectedObjectId}
            view={view}
          />
          <SupportingObjects
            actorProfiles={actorProfiles}
            currentPubkey={currentPubkey}
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
          actorProfiles={actorProfiles}
          currentPubkey={currentPubkey}
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
  const [initializationDraft, setInitializationDraft] = React.useState(
    createProjectViewInitializationDraft,
  );
  const [initializationConflict, setInitializationConflict] = React.useState<
    Extract<ProjectViewMutationResult, { status: "conflict" }> | undefined
  >();
  const query = useProjectViewQuery();
  const relayConnection = useRelayConnection();
  const relayPubkey =
    query.data?.status === "ready" || query.data?.status === "uninitialized"
      ? query.data.relayPubkey
      : undefined;
  const snapshotUpdatedAt =
    query.data?.status === "ready" ? query.data.updatedAt : undefined;
  const liveStatus = useProjectViewLiveSync({
    relayPubkey,
    snapshotUpdatedAt,
  });
  const degraded = isRelayConnectionDegraded(relayConnection);
  const verifiedRevision =
    query.data?.status === "ready" ? query.data.projectRevision : undefined;
  const refreshError =
    query.isError && query.data
      ? query.error instanceof Error
        ? query.error.message
        : "The latest Project View snapshot could not be verified."
      : undefined;
  const syncState: "refreshing" | "stale" | undefined = degraded
    ? "stale"
    : refreshError || liveStatus === "retrying"
      ? "stale"
      : query.data && (query.isFetching || liveStatus === "connecting")
        ? "refreshing"
        : undefined;
  const syncMessage =
    verifiedRevision === undefined || syncState === undefined
      ? undefined
      : degraded
        ? `Showing verified project revision ${verifiedRevision}. It may be stale while the Relay connection recovers.`
        : refreshError
          ? `Showing verified project revision ${verifiedRevision}. The latest refresh failed: ${refreshError}`
          : liveStatus === "retrying"
            ? `Showing verified project revision ${verifiedRevision} while the live update subscription reconnects.`
            : `Keeping verified project revision ${verifiedRevision} visible while a new complete snapshot is verified.`;

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
          {!degraded &&
          (query.data?.status === "ready" ||
            query.data?.status === "uninitialized") ? (
            <Badge variant="outline">Editable</Badge>
          ) : null}
          {query.data?.status === "ready" ? (
            <Badge variant="success">
              <ShieldCheck className="mr-1 h-3 w-3" />
              Verified
            </Badge>
          ) : null}
          {degraded && relayPubkey ? (
            <Badge variant="warning">
              <WifiOff className="mr-1 h-3 w-3" />
              Offline · may be stale
            </Badge>
          ) : null}
          {!degraded &&
          relayPubkey &&
          (query.isFetching || liveStatus === "connecting") ? (
            <Badge variant="secondary">
              <RefreshCw className="mr-1 h-3 w-3 animate-spin" />
              Syncing
            </Badge>
          ) : null}
          {!degraded && liveStatus === "retrying" ? (
            <Badge variant="warning">Live sync retrying</Badge>
          ) : null}
        </header>
      </TopChromeInsetHeader>

      {query.isPending ? <ProjectViewLoadingState /> : null}
      {query.isError && !query.data ? (
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
        <ProjectViewInitialize
          draft={initializationDraft}
          onApplied={() => {
            setInitializationDraft(createProjectViewInitializationDraft());
            setInitializationConflict(undefined);
          }}
          onChange={setInitializationDraft}
          onConflict={setInitializationConflict}
          onDiscardAndOpenLatest={async () => {
            setInitializationDraft(createProjectViewInitializationDraft());
            setInitializationConflict(undefined);
            await query.refetch();
          }}
        />
      ) : null}
      {query.data?.status === "ready" ? (
        <ReadyProjectView
          {...query.data}
          initializationConflict={initializationConflict}
          initializationDraft={
            isProjectViewInitializationDraftDirty(initializationDraft)
              ? initializationDraft
              : undefined
          }
          onDiscardInitializationDraft={() => {
            setInitializationDraft(createProjectViewInitializationDraft());
            setInitializationConflict(undefined);
          }}
          onRefresh={async () => {
            await query.refetch();
          }}
          onSelectObject={onSelectObject}
          selectedObjectId={selectedObjectId}
          syncMessage={syncMessage}
          syncState={syncState}
        />
      ) : null}
    </div>
  );
}
