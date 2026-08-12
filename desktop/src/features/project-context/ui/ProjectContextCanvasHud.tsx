import { Focus, Maximize2, Minus, Plus, Route, X } from "lucide-react";

import type { ProjectContextGraphModel } from "@/features/project-context/graph";
import type { ProjectContextLayout } from "@/features/project-context/layout";
import type { ProjectContextCanvasInsets } from "@/features/project-context/projectContextViewport";
import type {
  ProjectContextSemanticFreshness,
  ProjectContextSemanticOverlay,
} from "@/features/project-context/semanticOverlay";
import { Button } from "@/shared/ui/button";

/** Closed set of overlays that contribute to safe-area measurement. */
export type ProjectContextChromeContributor =
  | "summary"
  | "selection"
  | "controls"
  | "guidance";

type ChromeRef = (element: HTMLDivElement | null) => void;

/** Compact, content-safe controls and status layered over the graph canvas. */
export function ProjectContextCanvasHud({
  contextDocumentCount,
  externalInsets,
  graph,
  layout,
  onClearSemanticResult,
  onFitAll,
  onFitIsland,
  onFitSelection,
  onFitSemanticPaths,
  onZoomIn,
  onZoomOut,
  registerChromeContributor,
  selectedLabel,
  semanticFreshness,
  semanticOverlay,
}: {
  contextDocumentCount: number;
  externalInsets: ProjectContextCanvasInsets;
  graph: ProjectContextGraphModel;
  layout: ProjectContextLayout;
  onClearSemanticResult?: () => void;
  onFitAll: () => void;
  onFitIsland: (island: ProjectContextLayout["islands"][number]) => void;
  onFitSelection?: () => void;
  onFitSemanticPaths?: () => void;
  onZoomIn: () => void;
  onZoomOut: () => void;
  registerChromeContributor: (
    contributor: ProjectContextChromeContributor,
  ) => ChromeRef;
  selectedLabel?: string;
  semanticFreshness: ProjectContextSemanticFreshness;
  semanticOverlay: ProjectContextSemanticOverlay | null;
}) {
  const edgeLabel = graph.isAllContext
    ? graph.hubs.length === 1
      ? "edge"
      : "edges"
    : graph.hubs.length === 1
      ? "matching edge"
      : "matching edges";
  const right = externalInsets.right + 12;
  const bottom = externalInsets.bottom + 12;

  return (
    <div
      className="pointer-events-none absolute inset-0 z-20"
      data-testid="project-context-canvas-hud"
    >
      <div
        className="pointer-events-auto absolute left-3 top-3 max-w-xl rounded-xl border border-border/70 bg-background/90 px-3 py-2 shadow-lg backdrop-blur"
        data-context-document-count={contextDocumentCount}
        data-coordinate-count={graph.coordinates.length}
        data-edge-count={graph.hubs.length}
        data-project-context-chrome-contributor="summary"
        data-testid="project-context-result-counts"
        ref={registerChromeContributor("summary")}
        style={{
          left: externalInsets.left + 12,
          maxWidth: `calc(100% - ${externalInsets.left + externalInsets.right + 24}px)`,
        }}
      >
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <div
            className="min-w-0 text-sm font-semibold"
            data-testid={
              graph.isAllContext
                ? "project-context-island-summary"
                : "project-context-query-summary"
            }
          >
            {graph.isAllContext
              ? `${graph.islands.length} context ${graph.islands.length === 1 ? "island" : "islands"} · `
              : ""}
            {graph.coordinates.length} coordinates · {graph.hubs.length}{" "}
            {edgeLabel} · {contextDocumentCount} context docs
          </div>
          <div className="flex max-w-full items-center gap-1 overflow-x-auto">
            {graph.isAllContext
              ? layout.islands.map((island) => (
                  <Button
                    aria-label={`Fit Island ${island.index}`}
                    data-testid={`project-context-fit-island-${island.index}`}
                    key={island.stableKey}
                    onClick={() => onFitIsland(island)}
                    size="xs"
                    type="button"
                    variant="ghost"
                  >
                    <Focus />
                    Island {island.index}
                  </Button>
                ))
              : null}
            <Button
              data-testid="project-context-fit-all"
              onClick={onFitAll}
              size="xs"
              type="button"
              variant="ghost"
            >
              <Maximize2 />
              {graph.isAllContext ? "Fit all" : "Fit query"}
            </Button>
          </div>
        </div>
        <p className="mt-0.5 truncate text-xs text-muted-foreground">
          {graph.isAllContext
            ? graph.islands.length > 1
              ? `The current Project Context contains ${graph.islands.length} disconnected components.`
              : "All visible Context Edges form one connected component."
            : graph.hubs.length === 0
              ? "The query Anchors are shown for orientation. They are not a Context Island or a Gap."
              : "This focused result shares its Query Anchors; it is not a project-level Island count."}
        </p>
        {semanticOverlay ? (
          <div
            className="mt-1.5 flex flex-wrap items-center gap-1.5 border-t border-border/60 pt-1.5 text-2xs font-medium"
            data-testid="project-context-semantic-session-hud"
          >
            <span
              aria-hidden
              className={`h-2.5 w-2.5 rounded-full border-2 ${
                semanticFreshness === "stale"
                  ? "border-amber-600 dark:border-amber-300"
                  : "border-cyan-600 dark:border-cyan-300"
              }`}
            />
            <span data-testid="project-context-semantic-legend">
              {semanticFreshness === "stale"
                ? "Stale semantic snapshot"
                : "Semantic paths"}
            </span>
            <span>
              · {semanticOverlay.pathCount}{" "}
              {semanticOverlay.pathCount === 1 ? "path" : "paths"}
            </span>
            <span>
              · {semanticOverlay.rootCount}{" "}
              {semanticOverlay.rootCount === 1 ? "root" : "roots"}
            </span>
            <span>· Revision {semanticOverlay.projectContextRevision}</span>
            {semanticOverlay.partialCoverage ? (
              <span data-testid="project-context-semantic-partial-coverage">
                · Partial coverage
              </span>
            ) : null}
            {semanticOverlay.budgetExhausted ? (
              <span data-testid="project-context-semantic-budget-exhausted">
                · Budget exhausted
              </span>
            ) : null}
            {onFitSemanticPaths ? (
              <Button
                aria-label="Fit semantic paths"
                data-testid="project-context-fit-semantic-paths"
                onClick={onFitSemanticPaths}
                size="xs"
                type="button"
                variant="ghost"
              >
                <Route />
                Fit paths
              </Button>
            ) : (
              <Button
                aria-label="Fit semantic paths unavailable while the snapshot is stale"
                data-testid="project-context-fit-semantic-paths"
                disabled
                size="xs"
                type="button"
                variant="ghost"
              >
                <Route />
                Fit paths
              </Button>
            )}
            {onClearSemanticResult ? (
              <Button
                aria-label="Clear semantic result"
                data-testid="project-context-clear-semantic-result"
                onClick={onClearSemanticResult}
                size="xs"
                type="button"
                variant="ghost"
              >
                <X />
                Clear
              </Button>
            ) : null}
          </div>
        ) : null}
        {selectedLabel ? (
          <div
            className="mt-1.5 truncate border-t border-border/60 pt-1.5 text-xs font-medium"
            data-project-context-chrome-contributor="selection"
            data-testid="project-context-selection-status"
            ref={registerChromeContributor("selection")}
          >
            {selectedLabel}
          </div>
        ) : null}
      </div>

      <div
        className="pointer-events-auto absolute flex items-center gap-1 rounded-xl border border-border/70 bg-background/90 p-1 shadow-lg backdrop-blur"
        data-project-context-chrome-contributor="controls"
        ref={registerChromeContributor("controls")}
        style={{ bottom, right }}
      >
        <Button
          aria-label="Zoom out"
          onClick={onZoomOut}
          size="icon-xs"
          type="button"
          variant="ghost"
        >
          <Minus />
        </Button>
        <Button
          aria-label="Zoom in"
          onClick={onZoomIn}
          size="icon-xs"
          type="button"
          variant="ghost"
        >
          <Plus />
        </Button>
        <Button
          aria-label={
            graph.isAllContext ? "Fit all Context Islands" : "Fit query result"
          }
          data-testid="project-context-fit-all-canvas"
          onClick={onFitAll}
          size="icon-xs"
          type="button"
          variant="ghost"
        >
          <Maximize2 />
        </Button>
        {onFitSelection ? (
          <Button
            aria-label="Fit selected graph item"
            data-testid="project-context-fit-selection"
            onClick={onFitSelection}
            size="icon-xs"
            type="button"
            variant="ghost"
          >
            <Focus />
          </Button>
        ) : null}
      </div>

      <div
        className="absolute flex"
        data-project-context-chrome-contributor="guidance"
        ref={registerChromeContributor("guidance")}
        style={{
          bottom,
          left: externalInsets.left + 12,
          maxWidth: `calc(100% - ${externalInsets.left + externalInsets.right + 176}px)`,
        }}
      >
        <span className="truncate rounded-full bg-background/75 px-2.5 py-1 text-2xs text-muted-foreground shadow-sm backdrop-blur">
          Pan · Scroll to zoom · Undirected · placement carries no rank or
          causality
        </span>
      </div>
    </div>
  );
}
