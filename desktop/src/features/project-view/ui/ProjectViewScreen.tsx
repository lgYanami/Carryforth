import * as React from "react";
import {
  ChevronRight,
  LayoutDashboard,
  PanelRightClose,
  PanelRightOpen,
  RefreshCw,
  ShieldCheck,
  WifiOff,
} from "lucide-react";

import {
  identityFromMeta,
  useProjectDocumentLiveSync,
  useProjectDocumentMeta,
  useProjectDocuments,
} from "@/features/project-documents/hooks";
import {
  buildProjectViewExplorerModel,
  buildProjectViewExplorerPage,
  canonicalObjectOccurrenceKey,
  indexProjectDocumentCatalog,
  projectViewCanonicalParent,
  projectViewExplorerFallbackObjectIds,
  resolveProjectViewExplorerSelection,
  type ProjectViewExplorerSelection,
} from "@/features/project-view/explorerModel";
import type { ProjectViewCreateContext } from "@/features/project-view/model";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  useProjectViewLiveSync,
  useProjectViewQuery,
} from "@/features/project-view/hooks";
import { useProjectViewActors } from "@/features/project-view/useProjectViewActors";
import {
  canGovernProjectRole,
  projectRoleGovernanceCapabilities,
} from "@/features/project-view/projectRoleGovernance";
import { projectViewRoleLifecycleState } from "@/features/project-view/projectViewRoleLifecycle";
import { ProjectViewContextManagementDialog } from "@/features/project-view/ui/ProjectViewContextManagementDialog";
import { ProjectViewCurrentDocument } from "@/features/project-view/ui/ProjectViewCurrentDocument";
import { ProjectViewCurrentObject } from "@/features/project-view/ui/ProjectViewCurrentObject";
import { ProjectViewDeleteDialog } from "@/features/project-view/ui/ProjectViewDeleteDialog";
import {
  ProjectViewObjectActions,
  ProjectViewObjectMaintenance,
} from "@/features/project-view/ui/ProjectViewObjectMaintenance";
import { ProjectViewObjectDialog } from "@/features/project-view/ui/ProjectViewObjectDialog";
import { ProjectViewOutlinePanel } from "@/features/project-view/ui/ProjectViewOutlinePanel";
import { ProjectViewV3SetupGuide } from "@/features/project-view/ui/ProjectViewV3SetupGuide";
import {
  ProjectViewErrorState,
  ProjectViewForbiddenState,
  ProjectViewIntegrityFailureState,
  ProjectViewLoadingState,
  ProjectViewUnsupportedState,
} from "@/features/project-view/ui/ProjectViewStates";
import { isProjectViewIntegrityError } from "@/shared/api/tauriProjectView";
import type {
  ProjectViewLoadResult,
  ProjectViewObject,
  ProjectViewObjectType,
} from "@/shared/api/tauriProjectView";
import {
  isRelayConnectionDegraded,
  useRelayConnection,
} from "@/shared/api/useRelayConnection";
import { useIsAuxiliaryPanelOverlay } from "@/shared/hooks/use-mobile";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

type ProjectViewScreenProps = {
  onOpenDocument: (search: { document: string; revision?: number }) => void;
  onOpenOverview?: () => void;
  onSelectItem: (
    selection?: ProjectViewExplorerSelection,
    options?: { replace?: boolean },
  ) => void;
  onShowInProjectContext?: (object: ProjectViewObject) => void;
  selection?: ProjectViewExplorerSelection;
};

function selectionRequestKey(
  selection: ProjectViewExplorerSelection | undefined,
): string {
  if (!selection) return "profile";
  return selection.kind === "object"
    ? `object:${selection.objectId}:${selection.via ?? "canonical"}`
    : `document:${selection.documentId}:${selection.revision ?? "current"}:${selection.via ?? "canonical"}`;
}

function ReadyProjectView({
  activeObjectCount,
  onOpenDocument,
  onOutlineOpenChange,
  onSelectItem,
  onShowInProjectContext,
  outlineOpen,
  projectRevision,
  projectionGeneration,
  relayPubkey,
  roleContinuity,
  contextCapability,
  onRefresh,
  selection,
  syncMessage,
  syncState,
  updatedAt,
  view,
}: Extract<ProjectViewLoadResult, { status: "ready" }> &
  ProjectViewScreenProps & {
    onRefresh: () => Promise<unknown>;
    onOutlineOpenChange: (open: boolean) => void;
    outlineOpen: boolean;
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
  const [deleteTarget, setDeleteTarget] = React.useState<{
    object: ProjectViewObject;
    returnObjectId?: string;
  }>();
  const [contextTarget, setContextTarget] = React.useState<ProjectViewObject>();
  const headingRef = React.useRef<HTMLHeadingElement>(null);
  const focusMainAfterNavigation = React.useRef(false);
  const lastValidLocation = React.useRef<
    | {
        requestKey: string;
        fallbackObjectIds: string[];
      }
    | undefined
  >(undefined);
  const isOverlay = useIsAuxiliaryPanelOverlay();
  const documentMetaQuery = useProjectDocumentMeta();
  const documentsQuery = useProjectDocuments(documentMetaQuery.data);
  useProjectDocumentLiveSync(documentMetaQuery.data);
  const documentIdentity = documentMetaQuery.data
    ? identityFromMeta(documentMetaQuery.data)
    : undefined;
  const documentCatalog = React.useMemo(
    () => indexProjectDocumentCatalog(documentsQuery.data?.documents),
    [documentsQuery.data?.documents],
  );
  const explorer = React.useMemo(
    () => buildProjectViewExplorerModel({ view, documentCatalog }),
    [documentCatalog, view],
  );
  const resolvedSelection = React.useMemo(
    () => resolveProjectViewExplorerSelection(explorer, selection),
    [explorer, selection],
  );
  const page = React.useMemo(
    () => buildProjectViewExplorerPage(explorer, selection),
    [explorer, selection],
  );
  const requestKey = selectionRequestKey(selection);
  const { actorProfiles, currentPubkey, objectsById } = useProjectViewActors(
    view,
    roleContinuity,
  );
  const roleGovernance = React.useMemo(
    () => projectRoleGovernanceCapabilities(roleContinuity, currentPubkey),
    [currentPubkey, roleContinuity],
  );
  const selectedObject =
    page.kind === "object" ? page.currentObject : undefined;
  const selectedRoleDefinition =
    selectedObject?.objectType === "role"
      ? roleContinuity?.roles.find(
          (definition) => definition.roleId === selectedObject.id,
        )
      : undefined;
  const selectedRoleCanGovern = selectedRoleDefinition
    ? canGovernProjectRole(roleGovernance, selectedRoleDefinition.level)
    : false;
  const selectedObjectCanMaintain = Boolean(
    selectedObject &&
      (selectedObject.objectType !== "role" || selectedRoleCanGovern),
  );
  const selectedRoleLifecycle = React.useMemo(
    () => projectViewRoleLifecycleState(selectedRoleDefinition, roleContinuity),
    [roleContinuity, selectedRoleDefinition],
  );

  const commitSelection = React.useCallback(
    (next?: ProjectViewExplorerSelection, options?: { replace?: boolean }) => {
      if (next?.kind === "object" && next.objectId === view.profile.id) {
        onSelectItem(undefined, options);
        return;
      }
      if (
        next?.kind === "object" &&
        next.via === canonicalObjectOccurrenceKey(next.objectId)
      ) {
        onSelectItem({ kind: "object", objectId: next.objectId }, options);
        return;
      }
      onSelectItem(next, options);
    },
    [onSelectItem, view.profile.id],
  );

  React.useEffect(() => {
    if (!selection) return;
    if (selection.kind === "object" && selection.objectId === view.profile.id) {
      onSelectItem(undefined, { replace: true });
      return;
    }
    const canonicalVia =
      selection.kind === "object" &&
      selection.via === canonicalObjectOccurrenceKey(selection.objectId);
    if (resolvedSelection.resolution === "canonicalized" || canonicalVia) {
      commitSelection({ ...selection, via: undefined }, { replace: true });
    } else if (resolvedSelection.resolution === "fallback") {
      const previous = lastValidLocation.current;
      const fallbackObjectId =
        previous?.requestKey === requestKey
          ? previous.fallbackObjectIds.find((objectId) =>
              explorer.objectsById.has(objectId),
            )
          : undefined;
      commitSelection(
        fallbackObjectId
          ? { kind: "object", objectId: fallbackObjectId }
          : undefined,
        { replace: true },
      );
    }
  }, [
    commitSelection,
    explorer.objectsById,
    onSelectItem,
    requestKey,
    resolvedSelection.resolution,
    selection,
    view.profile.id,
  ]);

  React.useEffect(() => {
    if (resolvedSelection.resolution === "fallback") return;
    lastValidLocation.current = {
      requestKey,
      fallbackObjectIds: projectViewExplorerFallbackObjectIds(explorer, page),
    };
  }, [explorer, page, requestKey, resolvedSelection.resolution]);

  React.useEffect(() => {
    if (contextTarget && contextTarget.id !== selectedObject?.id) {
      setContextTarget(undefined);
    }
  }, [contextTarget, selectedObject?.id]);

  React.useEffect(() => {
    if (!focusMainAfterNavigation.current) return;
    focusMainAfterNavigation.current = false;
    window.requestAnimationFrame(() => {
      if (headingRef.current?.dataset.occurrenceKey === page.occurrenceKey) {
        headingRef.current.focus();
      }
    });
  }, [page.occurrenceKey]);

  const navigateFromMain = React.useCallback(
    (next: ProjectViewExplorerSelection) => {
      setContextTarget(undefined);
      focusMainAfterNavigation.current = true;
      commitSelection(next);
    },
    [commitSelection],
  );
  const navigateFromOutline = React.useCallback(
    (next: ProjectViewExplorerSelection) => {
      if (isOverlay) {
        onOutlineOpenChange(false);
        focusMainAfterNavigation.current = true;
      }
      commitSelection(next);
    },
    [commitSelection, isOverlay, onOutlineOpenChange],
  );
  function selectObjectFromContext(objectId: string) {
    setContextTarget(undefined);
    focusMainAfterNavigation.current = true;
    commitSelection({ kind: "object", objectId });
  }

  function requestDelete(object: ProjectViewObject) {
    const canonicalParent = projectViewCanonicalParent(explorer, object.id);
    setDeleteTarget({
      object,
      returnObjectId: canonicalParent?.objectId,
    });
  }

  function navigateAfterDelete() {
    commitSelection(
      deleteTarget?.returnObjectId
        ? { kind: "object", objectId: deleteTarget.returnObjectId }
        : undefined,
    );
  }

  return (
    <div className="relative flex min-h-0 flex-1 overflow-hidden">
      <main className="min-w-0 flex-1 overflow-y-auto">
        {syncMessage ? (
          <div className="mx-auto w-full max-w-6xl px-5 pt-5">
            <div
              className={
                syncState === "stale"
                  ? "flex min-w-0 items-start gap-2 rounded-xl border border-amber-500/40 bg-amber-500/10 px-3 py-2.5"
                  : "flex min-w-0 items-start gap-2 rounded-xl border border-border/70 bg-muted/30 px-3 py-2.5"
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
          </div>
        ) : null}

        {page.kind === "object" ? (
          <ProjectViewCurrentObject
            actions={
              <ProjectViewObjectActions
                canCreateRole={roleGovernance.canCreateMemberRole}
                canMaintain={selectedObjectCanMaintain}
                lifecycle={selectedRoleLifecycle}
                object={page.currentObject}
                onCreate={(initialType, context) =>
                  setEditor({ mode: "create", initialType, context })
                }
                onDelete={requestDelete}
                onEdit={(object) => setEditor({ mode: "edit", object })}
                onManageContext={setContextTarget}
                onShowInProjectContext={onShowInProjectContext}
              />
            }
            documentsLoading={
              documentMetaQuery.isPending ||
              (Boolean(documentMetaQuery.data) && documentsQuery.isPending)
            }
            headingRef={headingRef}
            onNavigate={navigateFromMain}
            page={page}
          >
            <ProjectViewObjectMaintenance
              actorProfiles={actorProfiles}
              currentPubkey={currentPubkey}
              lifecycle={selectedRoleLifecycle}
              object={page.currentObject}
              projectionGeneration={projectionGeneration}
              projectRevision={projectRevision}
              roleContinuity={roleContinuity}
              roleDefinition={selectedRoleDefinition}
            />
          </ProjectViewCurrentObject>
        ) : (
          <ProjectViewCurrentDocument
            actorProfiles={actorProfiles}
            currentPubkey={currentPubkey}
            headingRef={headingRef}
            identity={documentIdentity}
            identityLoading={documentMetaQuery.isPending}
            onNavigate={navigateFromMain}
            onOpenInDocuments={onOpenDocument}
            page={page}
          />
        )}

        <div className="mx-auto w-full max-w-6xl px-5 pb-8">
          <footer className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t border-border/70 pt-4 text-2xs text-muted-foreground">
            <span>{activeObjectCount} verified objects</span>
            <span>Project revision {projectRevision}</span>
            <span>Projection generation {projectionGeneration}</span>
            <span>Updated {new Date(updatedAt).toLocaleString()}</span>
            <span className="sr-only">Relay signer {relayPubkey}</span>
          </footer>
        </div>
      </main>
      {outlineOpen ? (
        <ProjectViewOutlinePanel
          currentOccurrenceKey={page.occurrenceKey}
          model={explorer}
          onClose={() => onOutlineOpenChange(false)}
          onNavigate={navigateFromOutline}
        />
      ) : null}
      <ProjectViewContextManagementDialog
        actingAssignmentId={
          contextTarget?.objectType === "role"
            ? roleGovernance.actingAssignmentId
            : undefined
        }
        canMutate={selectedObjectCanMaintain}
        contextCapability={contextCapability}
        object={contextTarget}
        objectsById={objectsById}
        onOpenChange={(open) => {
          if (!open) setContextTarget(undefined);
        }}
        onRefresh={onRefresh}
        onSelectObject={selectObjectFromContext}
        open={Boolean(contextTarget)}
        projectRevision={projectRevision}
      />
      {editor ? (
        <ProjectViewObjectDialog
          canCreateAdminRole={roleGovernance.canCreateAdminRole}
          canCreateRole={roleGovernance.canCreateMemberRole}
          canGovernRole={
            editor.mode === "edit" && editor.object.objectType === "role"
              ? selectedRoleCanGovern
              : true
          }
          context={editor.mode === "create" ? editor.context : undefined}
          initialType={
            editor.mode === "create" ? editor.initialType : undefined
          }
          mode={editor.mode}
          object={editor.mode === "edit" ? editor.object : undefined}
          onApplied={(objectId) => {
            if (objectId) {
              commitSelection({ kind: "object", objectId });
            }
          }}
          onOpenChange={(open) => {
            if (!open) setEditor(undefined);
          }}
          onReviewLatest={onRefresh}
          open
          projectRevision={projectRevision}
          roleHasActiveAssignment={
            editor.mode === "edit" && selectedRoleLifecycle.hasActiveAssignment
          }
          roleHasOpenProposal={
            editor.mode === "edit" && selectedRoleLifecycle.hasOpenProposal
          }
          roleHasResponsibleWork={
            editor.mode === "edit" && selectedRoleLifecycle.hasResponsibleWork
          }
          roleActingAssignmentId={roleGovernance.actingAssignmentId}
          view={view}
        />
      ) : null}
      <ProjectViewDeleteDialog
        actingAssignmentId={roleGovernance.actingAssignmentId}
        object={deleteTarget?.object}
        onDeleted={navigateAfterDelete}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(undefined);
        }}
        onReviewLatest={onRefresh}
        open={Boolean(deleteTarget)}
        projectRevision={projectRevision}
        roleLifecycle={
          deleteTarget?.object.objectType === "role"
            ? selectedRoleLifecycle
            : undefined
        }
        view={view}
        workHasActiveCommitment={Boolean(
          deleteTarget?.object.objectType === "work" &&
            roleContinuity?.commitments.some(
              (commitment) =>
                commitment.workId === deleteTarget.object.id &&
                !commitment.endedAt,
            ),
        )}
      />
    </div>
  );
}

export function ProjectViewScreen({
  onOpenDocument,
  onOpenOverview,
  onSelectItem,
  onShowInProjectContext,
  selection,
}: ProjectViewScreenProps) {
  const { activeCommunity } = useCommunities();
  const isOutlineOverlay = useIsAuxiliaryPanelOverlay();
  const [outlineOpen, setOutlineOpen] = React.useState(() => !isOutlineOverlay);
  const previousOverlay = React.useRef(isOutlineOverlay);
  const previousCommunityId = React.useRef(activeCommunity?.id);
  const query = useProjectViewQuery();
  const relayConnection = useRelayConnection();
  const relayPubkey =
    query.data?.status === "ready" ? query.data.relayPubkey : undefined;
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
  const fatalError = query.isError && !query.data ? query.error : undefined;
  const fatalErrorMessage =
    fatalError instanceof Error
      ? fatalError.message
      : "The Relay returned an unexpected Project View response.";

  React.useEffect(() => {
    if (previousOverlay.current === isOutlineOverlay) return;
    previousOverlay.current = isOutlineOverlay;
    setOutlineOpen(!isOutlineOverlay);
  }, [isOutlineOverlay]);

  React.useEffect(() => {
    const nextCommunityId = activeCommunity?.id;
    const previousId = previousCommunityId.current;
    previousCommunityId.current = nextCommunityId;
    if (!previousId || !nextCommunityId || previousId === nextCommunityId) {
      return;
    }
    onSelectItem(undefined, { replace: true });
    setOutlineOpen(!isOutlineOverlay);
  }, [activeCommunity?.id, isOutlineOverlay, onSelectItem]);

  return (
    <div className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
      <TopChromeInsetHeader flush>
        <header
          className="flex h-12 items-center gap-2 px-3 sm:gap-3 sm:px-5"
          data-tauri-drag-region
        >
          <Button
            className="-ml-2 min-w-0 max-w-48 shrink justify-start px-2"
            data-testid="return-community-overview"
            disabled={!onOpenOverview}
            onClick={onOpenOverview}
            size="sm"
            type="button"
            variant="ghost"
          >
            <LayoutDashboard />
            <span className="truncate">
              {activeCommunity?.name ?? "Community"}
            </span>
          </Button>
          <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-semibold">
              Full Project View
            </div>
            <div className="hidden text-2xs text-muted-foreground sm:block">
              Focused explorer and verified maintenance
            </div>
          </div>
          {!degraded && query.data?.status === "ready" ? (
            <Badge className="hidden sm:inline-flex" variant="outline">
              Editable
            </Badge>
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
          {query.data?.status === "ready" ? (
            <Button
              aria-label={
                outlineOpen ? "Hide Project Outline" : "Show Project Outline"
              }
              data-testid="project-view-outline-toggle"
              onClick={() => setOutlineOpen((open) => !open)}
              size="icon"
              title={
                outlineOpen ? "Hide Project Outline" : "Show Project Outline"
              }
              type="button"
              variant="ghost"
            >
              {outlineOpen ? <PanelRightClose /> : <PanelRightOpen />}
            </Button>
          ) : null}
        </header>
      </TopChromeInsetHeader>

      {query.isPending ? <ProjectViewLoadingState /> : null}
      {fatalError && isProjectViewIntegrityError(fatalError) ? (
        <ProjectViewIntegrityFailureState
          diagnostic={fatalErrorMessage}
          onRetry={() => void query.refetch()}
          retrying={query.isFetching}
        />
      ) : null}
      {fatalError && !isProjectViewIntegrityError(fatalError) ? (
        <ProjectViewErrorState
          message={fatalErrorMessage}
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
        <ProjectViewV3SetupGuide
          onRefresh={() => void query.refetch()}
          refreshing={query.isFetching}
          relayPubkey={query.data.relayPubkey}
        />
      ) : null}
      {query.data?.status === "ready" ? (
        <ReadyProjectView
          {...query.data}
          onOpenDocument={onOpenDocument}
          onOutlineOpenChange={setOutlineOpen}
          onRefresh={async () => {
            await query.refetch();
          }}
          onSelectItem={onSelectItem}
          onShowInProjectContext={onShowInProjectContext}
          outlineOpen={outlineOpen}
          selection={selection}
          syncMessage={syncMessage}
          syncState={syncState}
        />
      ) : null}
    </div>
  );
}
