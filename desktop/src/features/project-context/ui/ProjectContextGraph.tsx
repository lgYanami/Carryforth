import "@xyflow/react/dist/style.css";
import "./project-context-graph.css";

import { Focus, Maximize2, Minus, Plus, Route } from "lucide-react";
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
import { focusProjectContextGraphTarget } from "@/features/project-context/focus";
import {
  buildProjectContextLayoutTopology,
  layoutProjectContextGeometry,
  type layoutProjectContextGraph,
  materializeProjectContextLayout,
} from "@/features/project-context/layout";
import {
  buildProjectContextFlowElements,
  type ProjectContextFlowEdge,
  type ProjectContextFlowElements,
  type ProjectContextFlowNode,
  type ProjectContextGraphTarget,
} from "@/features/project-context/presentation";
import {
  type ProjectContextSemanticFreshness,
  type ProjectContextSemanticOverlay,
  semanticOverlayMatchesSubstrate,
} from "@/features/project-context/semanticOverlay";
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

function clearGraphHover(root: HTMLElement | null) {
  for (const element of root?.querySelectorAll("[data-context-graph-kind]") ??
    []) {
    element.removeAttribute("data-hover-emphasis");
  }
}

function applyGraphHover(
  root: HTMLElement | null,
  graph: ReturnType<typeof buildProjectContextGraph>,
  target: ProjectContextGraphTarget,
) {
  if (!root) return;
  const activeCoordinateKeys = new Set<string>();
  const activeEdgeKeys = new Set<string>();
  if (target.kind === "edge") {
    activeEdgeKeys.add(target.key);
    for (const key of graph.hubs.find((hub) => hub.edgeKey === target.key)
      ?.coordinateKeys ?? []) {
      activeCoordinateKeys.add(key);
    }
  } else {
    activeCoordinateKeys.add(target.key);
    for (const hub of graph.hubs) {
      if (hub.coordinateKeys.includes(target.key)) {
        activeEdgeKeys.add(hub.edgeKey);
      }
    }
  }

  for (const element of root.querySelectorAll("[data-context-graph-kind]")) {
    const kind = element.getAttribute("data-context-graph-kind");
    const coordinateKey = element.getAttribute("data-coordinate-key");
    const edgeKey = element.getAttribute("data-edge-key");
    const active =
      kind === "coordinate"
        ? coordinateKey !== null && activeCoordinateKeys.has(coordinateKey)
        : kind === "edge"
          ? edgeKey !== null && activeEdgeKeys.has(edgeKey)
          : target.kind === "edge"
            ? edgeKey === target.key
            : coordinateKey === target.key;
    element.setAttribute("data-hover-emphasis", active ? "active" : "dimmed");
  }
}

type ProjectContextGraphInnerProps = {
  elements: ProjectContextFlowElements;
  fitSemanticPathsRequest: number;
  focusSelectionRequest: number;
  graph: ReturnType<typeof buildProjectContextGraph>;
  layout: ReturnType<typeof layoutProjectContextGraph>;
  queryIdentity: string;
  selection: ProjectContextGraphTarget | null;
  semanticFreshness: ProjectContextSemanticFreshness;
  semanticOverlay: ProjectContextSemanticOverlay | null;
  textScale: number;
  onSelectionChange: (selection: ProjectContextGraphTarget | null) => void;
};

function ProjectContextGraphInner({
  elements,
  fitSemanticPathsRequest,
  focusSelectionRequest,
  graph,
  layout,
  onSelectionChange,
  queryIdentity,
  selection,
  semanticFreshness,
  semanticOverlay,
  textScale,
}: ProjectContextGraphInnerProps) {
  const shouldReduceMotion = useReducedMotion();
  const nodesInitialized = useNodesInitialized();
  const handledFocusSelectionRequest = React.useRef(0);
  const handledFitSemanticPathsRequest = React.useRef(0);
  const graphRootRef = React.useRef<HTMLElement>(null);
  const { fitBounds, fitView, getViewport, setViewport, zoomIn, zoomOut } =
    useReactFlow<ProjectContextFlowNode, ProjectContextFlowEdge>();
  const duration = shouldReduceMotion ? 0 : 220;
  const layoutKey = React.useMemo(
    () =>
      layout.nodes
        .map(
          (node) =>
            `${node.id}:${node.x}:${node.y}:${node.width}:${node.height}`,
        )
        .join(";"),
    [layout.nodes],
  );
  const semanticGeneration = semanticOverlay
    ? `${semanticOverlay.requestId}:${semanticOverlay.substrateIdentity}`
    : "none";
  const hoverResetKey = `${layoutKey}:${selection?.kind ?? "none"}:${selection?.key ?? "none"}:${semanticGeneration}`;
  const previousHoverResetKey = React.useRef<string | null>(null);
  const fittedQueryIdentity = React.useRef<string | null>(null);
  const fittedSemanticGeneration = React.useRef<string | null>(null);
  const previousLayout = React.useRef({ layout, queryIdentity, textScale });
  const semanticBounds = React.useMemo(() => {
    if (!semanticOverlay) return null;
    const targetIds = new Set(semanticOverlay.boundsTargetIds);
    const targets = layout.nodes.filter((node) => targetIds.has(node.id));
    if (targets.length === 0) return null;
    const minX = Math.min(...targets.map((node) => node.x));
    const minY = Math.min(...targets.map((node) => node.y));
    const maxX = Math.max(...targets.map((node) => node.x + node.width));
    const maxY = Math.max(...targets.map((node) => node.y + node.height));
    return { x: minX, y: minY, width: maxX - minX, height: maxY - minY };
  }, [layout.nodes, semanticOverlay]);
  const fitSemanticPaths = React.useCallback(() => {
    if (!semanticBounds) return;
    void fitBounds(semanticBounds, {
      padding: 0.24,
      duration,
    });
  }, [duration, fitBounds, semanticBounds]);

  React.useLayoutEffect(() => {
    const previous = previousLayout.current;
    previousLayout.current = { layout, queryIdentity, textScale };
    if (
      !nodesInitialized ||
      previous.queryIdentity !== queryIdentity ||
      previous.textScale === textScale ||
      previous.textScale <= 0
    ) {
      return;
    }
    const viewport = getViewport();
    const selectedId = selection
      ? selection.kind === "coordinate"
        ? `coordinate:${selection.key}`
        : `edge-hub:${selection.key}`
      : undefined;
    const previousNode = selectedId
      ? previous.layout.nodes.find((node) => node.id === selectedId)
      : undefined;
    const nextNode = selectedId
      ? layout.nodes.find((node) => node.id === selectedId)
      : undefined;
    if (previousNode && nextNode) {
      const previousCenterX = previousNode.x + previousNode.width / 2;
      const previousCenterY = previousNode.y + previousNode.height / 2;
      const nextCenterX = nextNode.x + nextNode.width / 2;
      const nextCenterY = nextNode.y + nextNode.height / 2;
      void setViewport(
        {
          x: viewport.x + (previousCenterX - nextCenterX) * viewport.zoom,
          y: viewport.y + (previousCenterY - nextCenterY) * viewport.zoom,
          zoom: viewport.zoom,
        },
        { duration: 0 },
      );
      return;
    }
    const root = graphRootRef.current;
    if (!root) return;
    const focusX = root.clientWidth / 2;
    const focusY = root.clientHeight / 2;
    const ratio = textScale / previous.textScale;
    const graphX = (focusX - viewport.x) / viewport.zoom;
    const graphY = (focusY - viewport.y) / viewport.zoom;
    void setViewport(
      {
        x: focusX - graphX * ratio * viewport.zoom,
        y: focusY - graphY * ratio * viewport.zoom,
        zoom: viewport.zoom,
      },
      { duration: 0 },
    );
  }, [
    getViewport,
    layout,
    nodesInitialized,
    queryIdentity,
    selection,
    setViewport,
    textScale,
  ]);

  React.useEffect(() => {
    if (
      !nodesInitialized ||
      layoutKey.length === 0 ||
      fittedQueryIdentity.current === queryIdentity
    ) {
      return;
    }
    fittedQueryIdentity.current = queryIdentity;
    if (semanticOverlay) return;
    void fitBounds(layout.bounds, { padding: 0.08, duration: 0 });
  }, [
    fitBounds,
    layout.bounds,
    layoutKey,
    nodesInitialized,
    queryIdentity,
    semanticOverlay,
  ]);

  React.useEffect(() => {
    if (
      !nodesInitialized ||
      !semanticBounds ||
      fittedSemanticGeneration.current === semanticGeneration
    ) {
      return;
    }
    fittedSemanticGeneration.current = semanticGeneration;
    fitSemanticPaths();
  }, [fitSemanticPaths, nodesInitialized, semanticBounds, semanticGeneration]);

  React.useEffect(() => {
    if (
      !nodesInitialized ||
      !semanticBounds ||
      fitSemanticPathsRequest === 0 ||
      fitSemanticPathsRequest === handledFitSemanticPathsRequest.current
    ) {
      return;
    }
    handledFitSemanticPathsRequest.current = fitSemanticPathsRequest;
    fitSemanticPaths();
  }, [
    fitSemanticPaths,
    fitSemanticPathsRequest,
    nodesInitialized,
    semanticBounds,
  ]);

  React.useEffect(() => {
    if (previousHoverResetKey.current === hoverResetKey) return;
    previousHoverResetKey.current = hoverResetKey;
    clearGraphHover(graphRootRef.current);
  }, [hoverResetKey]);

  React.useEffect(() => {
    if (
      !nodesInitialized ||
      focusSelectionRequest === 0 ||
      focusSelectionRequest === handledFocusSelectionRequest.current ||
      !selection
    ) {
      return;
    }
    const nodeId =
      selection.kind === "coordinate"
        ? `coordinate:${selection.key}`
        : `edge-hub:${selection.key}`;
    const node = layout.nodes.find((candidate) => candidate.id === nodeId);
    if (!node) return;
    handledFocusSelectionRequest.current = focusSelectionRequest;
    let cancelled = false;
    const focusTarget = () => {
      if (!cancelled) focusProjectContextGraphTarget(selection);
    };
    focusTarget();
    void fitBounds(
      { x: node.x, y: node.y, width: node.width, height: node.height },
      { padding: 0.8, duration },
    ).then(focusTarget);
    return () => {
      cancelled = true;
    };
  }, [
    duration,
    fitBounds,
    focusSelectionRequest,
    layout.nodes,
    nodesInitialized,
    selection,
  ]);

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
      if (target && !selection) {
        applyGraphHover(graphRootRef.current, graph, target);
      }
    },
    [graph, selection],
  );
  const handleNodeMouseLeave = React.useCallback<
    NodeMouseHandler<ProjectContextFlowNode>
  >(() => {
    clearGraphHover(graphRootRef.current);
  }, []);
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
        if (!selection) {
          applyGraphHover(graphRootRef.current, graph, {
            kind: "edge",
            key: edge.data.edgeKey,
          });
        }
      }
    },
    [graph, selection],
  );
  const handleEdgeMouseLeave = React.useCallback<
    EdgeMouseHandler<ProjectContextFlowEdge>
  >(() => {
    clearGraphHover(graphRootRef.current);
  }, []);
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
      aria-describedby={
        semanticOverlay
          ? "project-context-graph-description project-context-semantic-description"
          : "project-context-graph-description"
      }
      aria-label="Project Context graph canvas"
      className="project-context-graph relative min-h-0 flex-1 overflow-hidden bg-background/35"
      data-semantic-freshness={semanticOverlay ? semanticFreshness : undefined}
      data-semantic-overlay={semanticOverlay ? "active" : undefined}
      data-testid="project-context-graph"
      ref={graphRootRef}
    >
      <ReactFlow<ProjectContextFlowNode, ProjectContextFlowEdge>
        connectOnClick={false}
        deleteKeyCode={null}
        edges={elements.edges}
        edgesFocusable={false}
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
        onlyRenderVisibleElements
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
        {semanticOverlay ? (
          <div
            className="pointer-events-none absolute left-3 top-3 z-20 flex items-center gap-2 rounded-lg border border-border/70 bg-background/90 px-2.5 py-1.5 text-2xs font-medium shadow-sm backdrop-blur"
            data-testid="project-context-semantic-legend"
          >
            <span
              aria-hidden
              className={`h-2.5 w-2.5 rounded-full border-2 ${
                semanticFreshness === "stale"
                  ? "border-amber-600 dark:border-amber-300"
                  : "border-cyan-600 dark:border-cyan-300"
              }`}
            />
            {semanticFreshness === "stale"
              ? "Stale semantic snapshot"
              : "Semantic paths"}
          </div>
        ) : null}
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
          {selection ? (
            <Button
              aria-label="Fit selected graph item"
              data-testid="project-context-fit-selection"
              onClick={() => {
                const nodeId =
                  selection.kind === "coordinate"
                    ? `coordinate:${selection.key}`
                    : `edge-hub:${selection.key}`;
                const node = layout.nodes.find(
                  (candidate) => candidate.id === nodeId,
                );
                if (!node) return;
                focusProjectContextGraphTarget(selection);
                void fitBounds(
                  {
                    x: node.x,
                    y: node.y,
                    width: node.width,
                    height: node.height,
                  },
                  { padding: 0.8, duration },
                ).then(() => focusProjectContextGraphTarget(selection));
              }}
              size="icon-xs"
              type="button"
              variant="ghost"
            >
              <Focus />
            </Button>
          ) : null}
          {semanticOverlay && semanticBounds ? (
            <Button
              aria-label="Fit semantic paths"
              data-testid="project-context-fit-semantic-paths"
              onClick={fitSemanticPaths}
              size="icon-xs"
              type="button"
              variant="ghost"
            >
              <Route />
            </Button>
          ) : null}
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
      <span className="sr-only" id="project-context-graph-description">
        This is an undirected incidence graph. Node placement does not express
        source, target, order, importance, causality, or semantic similarity.
      </span>
      {semanticOverlay ? (
        <span className="sr-only" id="project-context-semantic-description">
          {semanticOverlay.pathCount} semantic paths are shown as one undirected
          highlighted subgraph with {semanticOverlay.edgeKeys.size} traversed
          Context Edges and {semanticOverlay.rootCount} candidate roots. Items
          outside the highlight are not declared irrelevant.
        </span>
      ) : null}
      <div className="pointer-events-none absolute inset-x-0 bottom-3 z-10 flex justify-center">
        <span className="rounded-full bg-background/75 px-2.5 py-1 text-2xs text-muted-foreground shadow-sm backdrop-blur">
          Pan · Scroll to zoom · Undirected · placement carries no rank or
          causality
        </span>
      </div>
      {selection ? (
        <div className="sr-only" aria-live="polite" aria-atomic="true">
          {selection.kind === "edge" ? "Context Edge" : "Coordinate"} selected.
        </div>
      ) : null}
      {semanticOverlay ? (
        <div className="sr-only" aria-live="polite" aria-atomic="true">
          Semantic result active. {semanticOverlay.pathCount} paths and{" "}
          {semanticOverlay.rootCount} roots shown.
        </div>
      ) : null}
    </section>
  );
}

/** Read-only query result graph; stable selection is owned by route state. */
export function ProjectContextGraph({
  fitSemanticPathsRequest = 0,
  focusSelectionRequest = 0,
  onSelectionChange,
  result,
  selection,
  semanticFreshness = "snapshot",
  semanticOverlay = null,
}: {
  fitSemanticPathsRequest?: number;
  focusSelectionRequest?: number;
  onSelectionChange: (
    selection: ProjectContextGraphTarget | null,
    options?: { replace?: boolean },
  ) => void;
  result: ProjectContextQueryResult;
  selection: ProjectContextGraphTarget | null;
  semanticFreshness?: ProjectContextSemanticFreshness;
  semanticOverlay?: ProjectContextSemanticOverlay | null;
}) {
  const textScale = useProjectContextTextScale();
  const visibleSemanticOverlay = React.useMemo(
    () =>
      semanticOverlay &&
      semanticOverlayMatchesSubstrate(semanticOverlay, result)
        ? semanticOverlay
        : null,
    [result, semanticOverlay],
  );
  const graph = React.useMemo(() => buildProjectContextGraph(result), [result]);
  const nextTopology = React.useMemo(
    () => buildProjectContextLayoutTopology(graph),
    [graph],
  );
  const topologyCache = React.useRef(nextTopology);
  if (topologyCache.current.descriptor !== nextTopology.descriptor) {
    topologyCache.current = nextTopology;
  }
  const topology = topologyCache.current;
  const geometry = React.useMemo(
    () => layoutProjectContextGeometry(topology),
    [topology],
  );
  const layout = React.useMemo(
    () => materializeProjectContextLayout(geometry, graph, textScale),
    [geometry, graph, textScale],
  );
  const elements = React.useMemo(
    () =>
      buildProjectContextFlowElements(
        graph,
        layout,
        selection,
        visibleSemanticOverlay,
      ),
    [graph, layout, selection, visibleSemanticOverlay],
  );
  const contextDocumentCount = new Set(
    graph.hubs.flatMap((hub) => hub.contextDocumentIds),
  ).size;

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
          fitSemanticPathsRequest={fitSemanticPathsRequest}
          focusSelectionRequest={focusSelectionRequest}
          graph={graph}
          layout={layout}
          onSelectionChange={onSelectionChange}
          queryIdentity={topology.queryIdentity}
          selection={selection}
          semanticFreshness={semanticFreshness}
          semanticOverlay={visibleSemanticOverlay}
          textScale={textScale}
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
