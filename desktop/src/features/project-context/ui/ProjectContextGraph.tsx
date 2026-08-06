import "@xyflow/react/dist/style.css";
import "./project-context-graph.css";

import { Focus, Maximize2, Minus, Plus } from "lucide-react";
import { useReducedMotion } from "motion/react";
import * as React from "react";
import {
  Background,
  BackgroundVariant,
  type EdgeMouseHandler,
  type NodeMouseHandler,
  type NodeTypes,
  ReactFlow,
  ReactFlowProvider,
  useNodesInitialized,
  useReactFlow,
  type EdgeTypes,
} from "@xyflow/react";

import { buildProjectContextGraph } from "@/features/project-context/graph";
import { layoutProjectContextGraph } from "@/features/project-context/layout";
import {
  buildProjectContextFlowElements,
  type ProjectContextFlowEdge,
  type ProjectContextFlowElements,
  type ProjectContextFlowNode,
  type ProjectContextGraphTarget,
} from "@/features/project-context/presentation";
import { ProjectContextCoordinateNode } from "@/features/project-context/ui/ProjectContextCoordinateNode";
import { ProjectContextEdgeHub } from "@/features/project-context/ui/ProjectContextEdgeHub";
import { ProjectContextIsland } from "@/features/project-context/ui/ProjectContextIsland";
import { ProjectContextSpoke } from "@/features/project-context/ui/ProjectContextSpoke";
import type { ProjectContextQueryResult } from "@/shared/api/tauriProjectContext";
import { Button } from "@/shared/ui/button";

const NODE_TYPES = {
  contextIsland: ProjectContextIsland,
  contextCoordinate: ProjectContextCoordinateNode,
  contextHub: ProjectContextEdgeHub,
} satisfies NodeTypes;

const EDGE_TYPES = {
  contextSpoke: ProjectContextSpoke,
} satisfies EdgeTypes;

function currentTextScale() {
  if (typeof document === "undefined") return 1;
  const fontSize = Number.parseFloat(
    window.getComputedStyle(document.documentElement).fontSize,
  );
  return Number.isFinite(fontSize) ? fontSize / 16 : 1;
}

function useProjectContextTextScale() {
  const [scale, setScale] = React.useState(currentTextScale);
  React.useLayoutEffect(() => {
    const update = () => setScale(currentTextScale());
    const observer = new MutationObserver(update);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "style"],
    });
    window.addEventListener("resize", update);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", update);
    };
  }, []);
  return scale;
}

function targetForNode(
  node: ProjectContextFlowNode,
): ProjectContextGraphTarget | null {
  if (node.data.kind === "coordinate") {
    return { kind: "coordinate", key: node.data.coordinate.coordinateKey };
  }
  if (node.data.kind === "hub") {
    return { kind: "edge", key: node.data.hub.edgeKey };
  }
  return null;
}

function sameTarget(
  left: ProjectContextGraphTarget | null,
  right: ProjectContextGraphTarget | null,
) {
  return left?.kind === right?.kind && left?.key === right?.key;
}

type ProjectContextGraphInnerProps = {
  elements: ProjectContextFlowElements;
  graph: ReturnType<typeof buildProjectContextGraph>;
  layout: ReturnType<typeof layoutProjectContextGraph>;
  selection: ProjectContextGraphTarget | null;
  setHovered: React.Dispatch<
    React.SetStateAction<ProjectContextGraphTarget | null>
  >;
  onSelectionChange: (selection: ProjectContextGraphTarget | null) => void;
};

function ProjectContextGraphInner({
  elements,
  graph,
  layout,
  onSelectionChange,
  selection,
  setHovered,
}: ProjectContextGraphInnerProps) {
  const shouldReduceMotion = useReducedMotion();
  const nodesInitialized = useNodesInitialized();
  const { fitView, zoomIn, zoomOut } = useReactFlow<
    ProjectContextFlowNode,
    ProjectContextFlowEdge
  >();
  const duration = shouldReduceMotion ? 0 : 220;
  const layoutKey = layout.nodes
    .map(
      (node) => `${node.id}:${node.x}:${node.y}:${node.width}:${node.height}`,
    )
    .join(";");

  React.useEffect(() => {
    if (!nodesInitialized || layoutKey.length === 0) return;
    void fitView({ padding: 0.08, duration: 0, maxZoom: 1.15 });
  }, [fitView, layoutKey, nodesInitialized]);

  const handleNodeClick = React.useCallback<
    NodeMouseHandler<ProjectContextFlowNode>
  >(
    (_event, node) => {
      const target = targetForNode(node);
      if (!target) return;
      onSelectionChange(sameTarget(selection, target) ? null : target);
    },
    [onSelectionChange, selection],
  );
  const handleNodeMouseEnter = React.useCallback<
    NodeMouseHandler<ProjectContextFlowNode>
  >(
    (_event, node) => {
      const target = targetForNode(node);
      if (target) setHovered(target);
    },
    [setHovered],
  );
  const handleNodeMouseLeave = React.useCallback<
    NodeMouseHandler<ProjectContextFlowNode>
  >(
    (_event, node) => {
      const target = targetForNode(node);
      setHovered((current) => (sameTarget(current, target) ? null : current));
    },
    [setHovered],
  );
  const handleEdgeClick = React.useCallback<
    EdgeMouseHandler<ProjectContextFlowEdge>
  >(
    (_event, edge) => {
      if (!edge.data) return;
      const target = { kind: "edge", key: edge.data.edgeKey } as const;
      onSelectionChange(sameTarget(selection, target) ? null : target);
    },
    [onSelectionChange, selection],
  );
  const handleEdgeMouseEnter = React.useCallback<
    EdgeMouseHandler<ProjectContextFlowEdge>
  >(
    (_event, edge) => {
      if (edge.data) {
        setHovered({ kind: "edge", key: edge.data.edgeKey });
      }
    },
    [setHovered],
  );
  const handleEdgeMouseLeave = React.useCallback<
    EdgeMouseHandler<ProjectContextFlowEdge>
  >(
    (_event, edge) => {
      if (!edge.data) return;
      const target = { kind: "edge", key: edge.data.edgeKey } as const;
      setHovered((current) => (sameTarget(current, target) ? null : current));
    },
    [setHovered],
  );
  const selectedLabel = React.useMemo(() => {
    if (!selection) return undefined;
    if (selection.kind === "coordinate") {
      return graph.coordinates.find(
        (coordinate) => coordinate.coordinateKey === selection.key,
      )?.displayTitle;
    }
    const hub = graph.hubs.find(
      (candidate) => candidate.edgeKey === selection.key,
    );
    return hub
      ? `Edge · ${hub.coordinateKeys.length} ${hub.coordinateKeys.length === 1 ? "coordinate" : "coordinates"} · ${hub.contextDocumentIds.length} ${hub.contextDocumentIds.length === 1 ? "doc" : "docs"}`
      : undefined;
  }, [graph, selection]);

  return (
    <section
      aria-label="Project Context graph canvas"
      className="project-context-graph relative min-h-0 flex-1 overflow-hidden bg-background/35"
      data-testid="project-context-graph"
    >
      <ReactFlow<ProjectContextFlowNode, ProjectContextFlowEdge>
        connectOnClick={false}
        deleteKeyCode={null}
        edges={elements.edges}
        edgesFocusable
        edgesReconnectable={false}
        edgeTypes={EDGE_TYPES}
        elementsSelectable
        fitView
        fitViewOptions={{ padding: 0.08, maxZoom: 1.15 }}
        maxZoom={2.2}
        minZoom={0.12}
        nodes={elements.nodes}
        nodesConnectable={false}
        nodesDraggable={false}
        nodesFocusable={false}
        nodeTypes={NODE_TYPES}
        onEdgeClick={handleEdgeClick}
        onEdgeMouseEnter={handleEdgeMouseEnter}
        onEdgeMouseLeave={handleEdgeMouseLeave}
        onNodeClick={handleNodeClick}
        onNodeMouseEnter={handleNodeMouseEnter}
        onNodeMouseLeave={handleNodeMouseLeave}
        onPaneClick={() => onSelectionChange(null)}
        panOnDrag
        proOptions={{ hideAttribution: true }}
        selectionOnDrag={false}
        zoomOnDoubleClick={false}
      >
        <Background
          color="hsl(var(--border) / 0.42)"
          gap={24}
          size={1}
          variant={BackgroundVariant.Dots}
        />
        <div className="absolute bottom-3 right-3 z-20 flex items-center gap-1 rounded-xl border border-border/70 bg-background/90 p-1 shadow-lg backdrop-blur">
          <Button
            aria-label="Zoom out"
            onClick={() => void zoomOut({ duration })}
            size="icon-xs"
            type="button"
            variant="ghost"
          >
            <Minus />
          </Button>
          <Button
            aria-label="Zoom in"
            onClick={() => void zoomIn({ duration })}
            size="icon-xs"
            type="button"
            variant="ghost"
          >
            <Plus />
          </Button>
          <Button
            aria-label={
              graph.isAllContext
                ? "Fit all Context Islands"
                : "Fit query result"
            }
            data-testid="project-context-fit-all-canvas"
            onClick={() =>
              void fitView({ padding: 0.08, duration, maxZoom: 1.15 })
            }
            size="icon-xs"
            type="button"
            variant="ghost"
          >
            <Maximize2 />
          </Button>
        </div>
        {selectedLabel ? (
          <div
            className="absolute right-3 top-3 z-20 max-w-64 truncate rounded-lg border border-border/70 bg-background/90 px-2.5 py-1.5 text-xs font-medium shadow-sm backdrop-blur"
            data-testid="project-context-selection-status"
            role="status"
          >
            {selectedLabel}
          </div>
        ) : null}
      </ReactFlow>
      <span className="sr-only">
        This is an undirected incidence graph. Node placement does not express
        source, target, order, importance, or causality.
      </span>
      <div className="pointer-events-none absolute inset-x-0 bottom-3 z-10 flex justify-center">
        <span className="rounded-full bg-background/75 px-2.5 py-1 text-2xs text-muted-foreground shadow-sm backdrop-blur">
          Pan to explore · Scroll to zoom · Undirected relationships
        </span>
      </div>
      <div className="sr-only" aria-live="polite">
        {selection
          ? `${selection.kind === "edge" ? "Context Edge" : "Coordinate"} selected.`
          : "Graph selection cleared."}
      </div>
    </section>
  );
}

/** Read-only query result graph; stable selection is owned by route state. */
export function ProjectContextGraph({
  onSelectionChange,
  result,
  selection,
}: {
  onSelectionChange: (
    selection: ProjectContextGraphTarget | null,
    options?: { replace?: boolean },
  ) => void;
  result: ProjectContextQueryResult;
  selection: ProjectContextGraphTarget | null;
}) {
  const textScale = useProjectContextTextScale();
  const graph = React.useMemo(() => buildProjectContextGraph(result), [result]);
  const layout = React.useMemo(
    () => layoutProjectContextGraph(graph, textScale),
    [graph, textScale],
  );
  const [hovered, setHovered] =
    React.useState<ProjectContextGraphTarget | null>(null);
  const activeTarget = selection ?? hovered;
  const elements = React.useMemo(
    () => buildProjectContextFlowElements(graph, layout, activeTarget),
    [activeTarget, graph, layout],
  );
  const contextDocumentCount = new Set(
    graph.hubs.flatMap((hub) => hub.contextDocumentIds),
  ).size;

  React.useEffect(() => {
    setHovered(null);
    if (!selection) return;
    const remainsVisible =
      selection.kind === "edge"
        ? graph.hubs.some((hub) => hub.edgeKey === selection.key)
        : graph.coordinates.some(
            (coordinate) => coordinate.coordinateKey === selection.key,
          );
    if (!remainsVisible) onSelectionChange(null, { replace: true });
  }, [graph, onSelectionChange, selection]);

  return (
    <ReactFlowProvider>
      <section className="flex min-h-96 flex-1 flex-col overflow-hidden rounded-2xl border border-border/70 bg-card/25">
        <div className="border-b border-border/70 bg-background/55 px-3 py-2.5 sm:px-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              {graph.isAllContext ? (
                <>
                  <div
                    className="text-sm font-semibold"
                    data-testid="project-context-island-summary"
                  >
                    {graph.islands.length} context{" "}
                    {graph.islands.length === 1 ? "island" : "islands"} ·{" "}
                    {graph.coordinates.length}{" "}
                    {graph.coordinates.length === 1
                      ? "coordinate"
                      : "coordinates"}{" "}
                    · {graph.hubs.length}{" "}
                    {graph.hubs.length === 1 ? "edge" : "edges"} ·{" "}
                    {contextDocumentCount} context{" "}
                    {contextDocumentCount === 1 ? "doc" : "docs"}
                  </div>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    {graph.islands.length > 1
                      ? `The current Project Context contains ${graph.islands.length} disconnected components.`
                      : "All visible Context Edges form one connected component."}
                  </p>
                </>
              ) : (
                <>
                  <div
                    className="text-sm font-semibold"
                    data-testid="project-context-query-summary"
                  >
                    {graph.hubs.length} matching{" "}
                    {graph.hubs.length === 1 ? "edge" : "edges"} ·{" "}
                    {graph.coordinates.length}{" "}
                    {graph.coordinates.length === 1
                      ? "coordinate"
                      : "coordinates"}{" "}
                    · {contextDocumentCount} context{" "}
                    {contextDocumentCount === 1 ? "doc" : "docs"}
                  </div>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    {graph.hubs.length === 0
                      ? "The query Anchors are shown for orientation. They are not a Context Island or a Gap."
                      : "This focused result shares its Query Anchors; it is not a project-level Island count."}
                  </p>
                </>
              )}
            </div>
            <IslandNavigation
              layout={layout}
              showIslands={graph.isAllContext}
            />
          </div>
        </div>
        <ProjectContextGraphInner
          elements={elements}
          graph={graph}
          layout={layout}
          onSelectionChange={onSelectionChange}
          selection={selection}
          setHovered={setHovered}
        />
      </section>
    </ReactFlowProvider>
  );
}

function IslandNavigation({
  layout,
  showIslands,
}: {
  layout: ReturnType<typeof layoutProjectContextGraph>;
  showIslands: boolean;
}) {
  const shouldReduceMotion = useReducedMotion();
  const { fitBounds, fitView } = useReactFlow<
    ProjectContextFlowNode,
    ProjectContextFlowEdge
  >();
  const duration = shouldReduceMotion ? 0 : 220;
  return (
    <div className="flex max-w-full items-center gap-1.5 overflow-x-auto pb-0.5">
      {showIslands
        ? layout.islands.map((island) => (
            <Button
              aria-label={`Fit Island ${island.index}`}
              data-testid={`project-context-fit-island-${island.index}`}
              key={island.stableKey}
              onClick={() =>
                void fitBounds(island.bounds, { padding: 0.14, duration })
              }
              size="xs"
              type="button"
              variant="outline"
            >
              <Focus />
              Island {island.index} · {island.edgeKeys.length}{" "}
              {island.edgeKeys.length === 1 ? "edge" : "edges"}
            </Button>
          ))
        : null}
      <Button
        data-testid="project-context-fit-all"
        onClick={() => void fitView({ padding: 0.08, duration, maxZoom: 1.15 })}
        size="xs"
        type="button"
        variant="ghost"
      >
        <Maximize2 />
        {showIslands ? "Fit all" : "Fit query"}
      </Button>
    </div>
  );
}
