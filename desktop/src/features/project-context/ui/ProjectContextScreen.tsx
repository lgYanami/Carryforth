import { FileText, Network, RefreshCw, ShieldCheck } from "lucide-react";

import {
  ALL_PROJECT_CONTEXT_QUERY,
  useProjectContextQuery,
} from "@/features/project-context/hooks";
import {
  projectContextErrorMessage,
  projectContextFailureKind,
  visibleContextDocumentCount,
} from "@/features/project-context/state";
import {
  ProjectContextEmptyState,
  ProjectContextFailureState,
  ProjectContextLoadingState,
} from "@/features/project-context/ui/ProjectContextStates";
import { ProjectContextGraph } from "@/features/project-context/ui/ProjectContextGraph";
import type { ProjectContextQueryResult } from "@/shared/api/tauriProjectContext";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

function ProjectContextGraphSlot({
  result,
}: {
  result: ProjectContextQueryResult;
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
              Context Edges
            </div>
            <div className="mt-1 text-xl font-semibold">
              {result.edges.length}
            </div>
          </div>
          <div className="rounded-xl border border-border/70 bg-card/60 p-3">
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Coordinates
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
          <ProjectContextGraph result={result} />
        </div>
      </div>
    </main>
  );
}

export function ProjectContextScreen() {
  const query = useProjectContextQuery(ALL_PROJECT_CONTEXT_QUERY);
  const result = query.data;
  const fatalError = query.isError && !result ? query.error : undefined;
  const refreshError = query.isError && result ? query.error : undefined;
  const refreshMessage = refreshError
    ? projectContextErrorMessage(refreshError)
    : undefined;

  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="project-context-screen"
    >
      <TopChromeInsetHeader flush>
        <header
          className="flex h-12 items-center gap-2 px-3 sm:gap-3 sm:px-5"
          data-tauri-drag-region
        >
          <Network className="h-4 w-4 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-semibold">
              Project Context
            </div>
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
          {refreshMessage ? <Badge variant="warning">Stale</Badge> : null}
          <Button
            aria-label="Refresh Project Context"
            data-testid="project-context-refresh"
            disabled={query.isFetching}
            onClick={() => void query.refetch()}
            size="icon"
            type="button"
            variant="ghost"
          >
            <RefreshCw
              className={`h-4 w-4 ${query.isFetching ? "animate-spin" : ""}`}
            />
          </Button>
        </header>
      </TopChromeInsetHeader>

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
      ) : result && query.isFetching ? (
        <div
          className="border-b border-border/70 bg-muted/20 px-4 py-2 text-xs text-muted-foreground"
          data-testid="project-context-refreshing"
          role="status"
        >
          Keeping verified Context revision {result.context.contextRevision}
          visible while a new complete snapshot is verified.
        </div>
      ) : null}

      {query.isPending ? <ProjectContextLoadingState /> : null}
      {fatalError ? (
        <ProjectContextFailureState
          diagnostic={projectContextErrorMessage(fatalError)}
          kind={projectContextFailureKind(fatalError)}
          onRetry={() => void query.refetch()}
          retrying={query.isFetching}
        />
      ) : null}
      {result && result.context.activeEdgeCount === 0 ? (
        <ProjectContextEmptyState />
      ) : null}
      {result && result.context.activeEdgeCount > 0 ? (
        <ProjectContextGraphSlot result={result} />
      ) : null}
    </div>
  );
}
