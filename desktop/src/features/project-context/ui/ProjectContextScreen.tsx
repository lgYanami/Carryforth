import {
  AlertTriangle,
  FileText,
  Network,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import * as React from "react";

import {
  useProjectDocumentMeta,
  useProjectDocuments,
} from "@/features/project-documents/hooks";
import {
  buildProjectContextCoordinateOptions,
  type ProjectContextCoordinateOption,
} from "@/features/project-context/queryModel";
import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import { isAllProjectContextQuery } from "@/features/project-context/routeState";
import {
  projectContextErrorMessage,
  projectContextFailureKind,
  visibleContextDocumentCount,
} from "@/features/project-context/state";
import { ProjectContextGraph } from "@/features/project-context/ui/ProjectContextGraph";
import {
  type ProjectContextPickerSourceState,
  ProjectContextQueryBar,
} from "@/features/project-context/ui/ProjectContextQueryBar";
import {
  ProjectContextEmptyState,
  ProjectContextFailureState,
  ProjectContextLoadingState,
} from "@/features/project-context/ui/ProjectContextStates";
import { useProjectContextQuery } from "@/features/project-context/hooks";
import { useProjectViewQuery } from "@/features/project-view/hooks";
import { indexProjectViewObjects } from "@/features/project-view/model";
import type {
  ProjectContextQuery,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

type ValidProjectContextScreenProps = {
  appliedQuery: ProjectContextQuery;
  onApplyQuery: (query: ProjectContextQuery) => void;
  onSelectionChange: (
    selection: ProjectContextRouteSelection | null,
    options?: { replace?: boolean },
  ) => void;
  selection: ProjectContextRouteSelection | null;
};

type InvalidProjectContextScreenProps = {
  onResetRoute: () => void;
  routeError: string;
};

function ProjectContextHeader({
  onRefresh,
  refreshing,
  result,
  stale,
}: {
  onRefresh?: () => void;
  refreshing?: boolean;
  result?: ProjectContextQueryResult;
  stale?: boolean;
}) {
  return (
    <TopChromeInsetHeader flush>
      <header
        className="flex h-12 items-center gap-2 px-3 sm:gap-3 sm:px-5"
        data-tauri-drag-region
      >
        <Network className="h-4 w-4 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">Project Context</div>
          <div className="hidden text-2xs text-muted-foreground sm:block">
            Verified, read-only relationships across project coordinates
          </div>
        </div>
        {result ? (
          <Badge variant="success">
            <ShieldCheck className="mr-1 h-3 w-3" />
            Verified
          </Badge>
        ) : null}
        {result && !result.context.capabilityEnabled ? (
          <Badge variant="warning">Capability off · read-only</Badge>
        ) : null}
        {result ? (
          <Badge className="hidden sm:inline-flex" variant="outline">
            Revision {result.context.contextRevision}
          </Badge>
        ) : null}
        {stale ? <Badge variant="warning">Stale</Badge> : null}
        {onRefresh ? (
          <Button
            aria-label="Refresh Project Context"
            data-testid="project-context-refresh"
            disabled={refreshing}
            onClick={onRefresh}
            size="icon"
            type="button"
            variant="ghost"
          >
            <RefreshCw
              className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`}
            />
          </Button>
        ) : null}
      </header>
    </TopChromeInsetHeader>
  );
}

function ProjectContextGraphSlot({
  onSelectionChange,
  result,
  selection,
}: {
  onSelectionChange: ValidProjectContextScreenProps["onSelectionChange"];
  result: ProjectContextQueryResult;
  selection: ProjectContextRouteSelection | null;
}) {
  const documentCount = visibleContextDocumentCount(result);
  return (
    <main className="min-h-0 flex-1 overflow-auto p-4 sm:p-6">
      <div className="mx-auto flex h-full min-h-80 max-w-6xl flex-col gap-4">
        <section
          className="grid gap-3 sm:grid-cols-3"
          data-context-document-count={documentCount}
          data-coordinate-count={result.coordinateDetails.length}
          data-edge-count={result.edges.length}
          data-testid="project-context-result-counts"
        >
          <div className="rounded-xl border border-border/70 bg-card/60 p-3">
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Matching Edges
            </div>
            <div className="mt-1 text-xl font-semibold">
              {result.edges.length}
            </div>
          </div>
          <div className="rounded-xl border border-border/70 bg-card/60 p-3">
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Visible Coordinates
            </div>
            <div className="mt-1 text-xl font-semibold">
              {result.coordinateDetails.length}
            </div>
          </div>
          <div className="rounded-xl border border-border/70 bg-card/60 p-3">
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Context Documents
            </div>
            <div className="mt-1 text-xl font-semibold">{documentCount}</div>
          </div>
        </section>
        <div
          className="flex min-h-96 flex-1"
          data-testid="project-context-graph-slot"
        >
          <ProjectContextGraph
            onSelectionChange={onSelectionChange}
            result={result}
            selection={selection}
          />
        </div>
      </div>
    </main>
  );
}

function pickerSourceState(input: {
  error: unknown;
  loading: boolean;
  ready: boolean;
}): ProjectContextPickerSourceState {
  if (input.ready) return "ready";
  if (input.error) return "unavailable";
  if (input.loading) return "loading";
  return "unavailable";
}

function ValidProjectContextScreen({
  appliedQuery,
  onApplyQuery,
  onSelectionChange,
  selection,
}: ValidProjectContextScreenProps) {
  const contextQuery = useProjectContextQuery(appliedQuery);
  const projectViewQuery = useProjectViewQuery();
  const documentMetaQuery = useProjectDocumentMeta();
  const documentsQuery = useProjectDocuments(documentMetaQuery.data);
  const result = contextQuery.data;
  const fatalError =
    contextQuery.isError && !result ? contextQuery.error : undefined;
  const refreshError =
    contextQuery.isError && result ? contextQuery.error : undefined;
  const refreshMessage = refreshError
    ? projectContextErrorMessage(refreshError)
    : undefined;
  const projectViewObjects = React.useMemo(
    () =>
      projectViewQuery.data?.status === "ready"
        ? [...indexProjectViewObjects(projectViewQuery.data.view).values()]
        : undefined,
    [projectViewQuery.data],
  );
  const coordinateOptions = React.useMemo<ProjectContextCoordinateOption[]>(
    () =>
      buildProjectContextCoordinateOptions({
        projectViewObjects,
        documents: documentsQuery.data?.documents,
        visibleDetails: result?.coordinateDetails,
      }),
    [
      documentsQuery.data?.documents,
      projectViewObjects,
      result?.coordinateDetails,
    ],
  );
  const projectViewState = pickerSourceState({
    error: projectViewQuery.error,
    loading: projectViewQuery.isPending,
    ready: projectViewQuery.data?.status === "ready",
  });
  const documentsState = pickerSourceState({
    error: documentMetaQuery.error ?? documentsQuery.error,
    loading:
      documentMetaQuery.isPending ||
      Boolean(documentMetaQuery.data && documentsQuery.isPending),
    ready: Boolean(documentMetaQuery.data && documentsQuery.data),
  });
  const allContext = isAllProjectContextQuery(appliedQuery);

  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="project-context-screen"
    >
      <ProjectContextHeader
        onRefresh={() => void contextQuery.refetch()}
        refreshing={contextQuery.isFetching}
        result={result}
        stale={Boolean(refreshMessage)}
      />

      {refreshMessage && result ? (
        <div
          className="flex items-start gap-2 border-b border-warning/30 bg-warning/10 px-4 py-2 text-xs text-muted-foreground"
          data-testid="project-context-stale-message"
          role="status"
        >
          <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>
            Showing verified Context revision {result.context.contextRevision}.
            The latest refresh failed: {refreshMessage}
          </span>
        </div>
      ) : result && contextQuery.isFetching ? (
        <div
          className="border-b border-border/70 bg-muted/20 px-4 py-2 text-xs text-muted-foreground"
          data-testid="project-context-refreshing"
          role="status"
        >
          Keeping verified Context revision {result.context.contextRevision}
          visible while a new complete snapshot is verified.
        </div>
      ) : null}

      <ProjectContextQueryBar
        appliedQuery={appliedQuery}
        coordinateOptions={coordinateOptions}
        documentsState={documentsState}
        onRun={onApplyQuery}
        projectViewState={projectViewState}
      />

      {contextQuery.isPending ? <ProjectContextLoadingState /> : null}
      {fatalError ? (
        <ProjectContextFailureState
          diagnostic={projectContextErrorMessage(fatalError)}
          kind={projectContextFailureKind(fatalError)}
          onRetry={() => void contextQuery.refetch()}
          retrying={contextQuery.isFetching}
        />
      ) : null}
      {result && allContext && result.context.activeEdgeCount === 0 ? (
        <ProjectContextEmptyState />
      ) : null}
      {result && (!allContext || result.context.activeEdgeCount > 0) ? (
        <ProjectContextGraphSlot
          onSelectionChange={onSelectionChange}
          result={result}
          selection={selection}
        />
      ) : null}
    </div>
  );
}

function InvalidProjectContextScreen({
  onResetRoute,
  routeError,
}: InvalidProjectContextScreenProps) {
  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="project-context-screen"
    >
      <ProjectContextHeader />
      <main
        className="flex min-h-0 flex-1 items-center justify-center p-6"
        data-testid="project-context-invalid-route"
      >
        <div className="max-w-lg text-center">
          <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-2xl border border-destructive/30 bg-destructive/10 text-destructive">
            <AlertTriangle className="h-5 w-5" />
          </div>
          <h1 className="mt-4 text-lg font-semibold">
            Project Context link is invalid
          </h1>
          <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
            The query or selection in this link was rejected before Desktop
            contacted the trusted Project Context boundary.
          </p>
          <code className="mt-4 block rounded-lg border border-border/70 bg-muted/20 px-3 py-2 text-left text-xs text-muted-foreground">
            {routeError}
          </code>
          <Button
            className="mt-4"
            data-testid="project-context-reset-invalid-route"
            onClick={onResetRoute}
            size="sm"
            type="button"
            variant="outline"
          >
            Open All Context
          </Button>
        </div>
      </main>
    </div>
  );
}

/** Stable route surface for valid query state or rejected deep links. */
export function ProjectContextScreen(
  props: ValidProjectContextScreenProps | InvalidProjectContextScreenProps,
) {
  return "routeError" in props ? (
    <InvalidProjectContextScreen {...props} />
  ) : (
    <ValidProjectContextScreen {...props} />
  );
}
