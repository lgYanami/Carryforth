import "@xyflow/react/dist/style.css";
import "./project-context-graph.css";

import { useReducedMotion } from "motion/react";
import * as React from "react";
import {
  Background,
  BackgroundVariant,
  type NodeTypes,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type EdgeTypes,
} from "@xyflow/react";

import { buildProjectContextGraph } from "@/features/project-context/graph";
import { focusProjectContextGraphTarget } from "@/features/project-context/focus";
import {
  buildProjectContextLayoutTopology,
  layoutProjectContextGeometry,
  materializeProjectContextLayout,
  type ProjectContextLayout,
} from "@/features/project-context/layout";
import {
  EMPTY_PROJECT_CONTEXT_CANVAS_INSETS,
  mergeProjectContextCanvasInsets,
  type ProjectContextCanvasInsets,
  recenterProjectContextViewportForTextScale,
} from "@/features/project-context/projectContextViewport";
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
import {
  ProjectContextCanvasHud,
  type ProjectContextChromeContributor,
} from "@/features/project-context/ui/ProjectContextCanvasHud";
import { ProjectContextCoordinateNode } from "@/features/project-context/ui/ProjectContextCoordinateNode";
import { ProjectContextEdgeHub } from "@/features/project-context/ui/ProjectContextEdgeHub";
import { ProjectContextIsland } from "@/features/project-context/ui/ProjectContextIsland";
import { ProjectContextSpoke } from "@/features/project-context/ui/ProjectContextSpoke";
import {
  clearProjectContextGraphHover,
  projectContextDocumentCount,
  projectContextSelectedLabel,
  useProjectContextPointerInteractions,
  useProjectContextTextScale,
} from "@/features/project-context/ui/projectContextGraphInteraction";
import { useProjectContextChromeMeasurement } from "@/features/project-context/ui/useProjectContextChromeMeasurement";
import {
  type ProjectContextFitQueueRequest,
  type ProjectContextFitRequest,
  useProjectContextFitSubmission,
} from "@/features/project-context/ui/useProjectContextFitSubmission";
import { useProjectContextHumanViewportAuthority } from "@/features/project-context/ui/useProjectContextHumanViewportAuthority";
import { useProjectContextResizePreservation } from "@/features/project-context/ui/useProjectContextResizePreservation";
import { useProjectContextViewportAuthority } from "@/features/project-context/ui/useProjectContextViewportAuthority";
import type { ProjectContextQueryResult } from "@/shared/api/tauriProjectContext";

const NODE_TYPES = {
  contextIsland: ProjectContextIsland,
  contextCoordinate: ProjectContextCoordinateNode,
  contextHub: ProjectContextEdgeHub,
} satisfies NodeTypes;

const EDGE_TYPES = {
  contextSpoke: ProjectContextSpoke,
} satisfies EdgeTypes;

// Dense project-wide Islands need an overview below the legacy card's 0.12
// floor; Humans can still zoom back in without changing canonical geometry.
const MIN_ZOOM = 0.05;
const MAX_ZOOM = 2.2;
const FIT_MAX_ZOOM = 1.15;

type ProjectContextGraphInnerProps = {
  elements: ProjectContextFlowElements;
  externalInsets: ProjectContextCanvasInsets;
  fitSemanticPathsRequest: number;
  fitSuspended: boolean;
  focusSelectionRequest: number;
  graph: ReturnType<typeof buildProjectContextGraph>;
  layout: ProjectContextLayout;
  onClearSemanticResult?: () => void;
  queryIdentity: string;
  selection: ProjectContextGraphTarget | null;
  semanticFreshness: ProjectContextSemanticFreshness;
  semanticOverlay: ProjectContextSemanticOverlay | null;
  semanticSessionOverlay: ProjectContextSemanticOverlay | null;
  textScale: number;
  onSelectionChange: (selection: ProjectContextGraphTarget | null) => void;
};

function ProjectContextGraphInner({
  elements,
  externalInsets,
  fitSemanticPathsRequest,
  fitSuspended,
  focusSelectionRequest,
  graph,
  layout,
  onClearSemanticResult,
  onSelectionChange,
  queryIdentity,
  selection,
  semanticFreshness,
  semanticOverlay,
  semanticSessionOverlay,
  textScale,
}: ProjectContextGraphInnerProps) {
  const shouldReduceMotion = useReducedMotion();
  const graphRootRef = React.useRef<HTMLElement>(null);
  const { getViewport, setViewport, zoomIn, zoomOut } = useReactFlow<
    ProjectContextFlowNode,
    ProjectContextFlowEdge
  >();
  const duration = shouldReduceMotion ? 0 : 220;
  const selectedLabel = React.useMemo(
    () => projectContextSelectedLabel(graph, selection),
    [graph, selection],
  );
  const expectedChromeContributors = React.useMemo<
    readonly ProjectContextChromeContributor[]
  >(
    () =>
      selectedLabel
        ? ["summary", "selection", "controls", "guidance"]
        : ["summary", "controls", "guidance"],
    [selectedLabel],
  );
  const { measurement, registerChromeContributor } =
    useProjectContextChromeMeasurement({
      expectedContributors: expectedChromeContributors,
      externalInsets,
      rootRef: graphRootRef,
    });
  const measurementGeneration = React.useRef(measurement.generation);
  measurementGeneration.current = measurement.generation;

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
  const queryIdentityRef = React.useRef(queryIdentity);
  const semanticGenerationRef = React.useRef(semanticGeneration);
  queryIdentityRef.current = queryIdentity;
  semanticGenerationRef.current = semanticGeneration;
  const hoverResetKey = `${layoutKey}:${selection?.kind ?? "none"}:${selection?.key ?? "none"}:${semanticGeneration}`;
  const previousHoverResetKey = React.useRef<string | null>(null);
  const previousLayout = React.useRef({ layout, queryIdentity, textScale });
  const handledQueryIdentity = React.useRef<string | null>(null);
  const queuedQueryIdentity = React.useRef<string | null>(null);
  const handledSemanticGeneration = React.useRef<string | null>(null);
  const queuedSemanticGeneration = React.useRef<string | null>(null);
  const handledFocusSelectionRequest = React.useRef(0);
  const queuedFocusSelectionRequest = React.useRef(0);
  const handledFitSemanticPathsRequest = React.useRef(0);
  const queuedFitSemanticPathsRequest = React.useRef(0);
  const [pendingFit, setPendingFit] =
    React.useState<ProjectContextFitRequest | null>(null);
  const [autoFitCount, setAutoFitCount] = React.useState(0);
  const [viewportEstablished, setViewportEstablished] = React.useState(false);
  const fitGeneration = React.useRef(0);
  const textScaleGeneration = React.useRef(0);
  const {
    armHumanInteractionFallback,
    authorityPending,
    beginAuthority,
    currentAuthority,
    currentHumanViewportGeneration,
    humanViewportGeneration,
    invalidateAuthority,
    settleAuthority,
    snapshot: viewportAuthoritySnapshot,
    trackOperation,
  } = useProjectContextViewportAuthority();
  const submittedFit = React.useRef<{
    authority: number;
    chromeGeneration: number;
  } | null>(null);
  const {
    correctionCount: viewportCorrectionCount,
    getBaselineSize: getResizeBaselineSize,
    resetBaseline: resetResizeBaseline,
  } = useProjectContextResizePreservation({
    authorityPending,
    fitGeneration,
    getViewport,
    humanViewportGeneration,
    queryIdentity: queryIdentityRef,
    rootRef: graphRootRef,
    setViewport,
    textScaleGeneration,
  });

  const queueFit = React.useCallback(
    (request: ProjectContextFitQueueRequest) => {
      const authority = beginAuthority();
      fitGeneration.current += 1;
      submittedFit.current = null;
      setPendingFit({
        ...request,
        authority,
        humanViewportGeneration: humanViewportGeneration.current,
        queryIdentity: queryIdentityRef.current,
        textScaleGeneration: textScaleGeneration.current,
      });
    },
    [beginAuthority, humanViewportGeneration],
  );

  const cancelPendingViewportOperation = React.useCallback(() => {
    submittedFit.current = null;
    setPendingFit(null);
    setViewportEstablished(true);
  }, []);
  const {
    beginGesture: beginHumanViewportGesture,
    continueGesture: continueHumanViewportGesture,
    endGesture: endHumanViewportGesture,
    runCommand: runHumanViewportCommand,
  } = useProjectContextHumanViewportAuthority({
    armHumanInteractionFallback,
    beginAuthority,
    cancelPendingViewportOperation,
    duration,
    resetResizeBaseline,
    settleAuthority,
    trackOperation,
  });

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

  const queueSemanticFit = React.useCallback(
    (completion: ProjectContextFitRequest["completion"]) => {
      if (!semanticBounds) return;
      queueFit({
        bounds: semanticBounds,
        completion,
        duration,
        maxZoom: FIT_MAX_ZOOM,
        padding: 0.24,
        semanticGeneration: semanticGenerationRef.current,
      });
    },
    [duration, queueFit, semanticBounds],
  );

  React.useLayoutEffect(() => {
    const previous = previousLayout.current;
    previousLayout.current = { layout, queryIdentity, textScale };
    if (
      previous.queryIdentity !== queryIdentity ||
      previous.textScale === textScale ||
      previous.textScale <= 0
    ) {
      return;
    }
    textScaleGeneration.current += 1;
    fitGeneration.current += 1;
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
    let nextViewport: ReturnType<typeof getViewport>;
    if (previousNode && nextNode) {
      const previousCenterX = previousNode.x + previousNode.width / 2;
      const previousCenterY = previousNode.y + previousNode.height / 2;
      const nextCenterX = nextNode.x + nextNode.width / 2;
      const nextCenterY = nextNode.y + nextNode.height / 2;
      nextViewport = {
        x: viewport.x + (previousCenterX - nextCenterX) * viewport.zoom,
        y: viewport.y + (previousCenterY - nextCenterY) * viewport.zoom,
        zoom: viewport.zoom,
      };
    } else {
      const root = graphRootRef.current;
      if (!root) return;
      const nextSize = {
        width: root.clientWidth,
        height: root.clientHeight,
      };
      nextViewport = recenterProjectContextViewportForTextScale({
        nextSize,
        previousSize: getResizeBaselineSize() ?? nextSize,
        scaleRatio:
          previous.layout.bounds.width > 0
            ? layout.bounds.width / previous.layout.bounds.width
            : textScale / previous.textScale,
        viewport,
      });
    }
    const authority = beginAuthority();
    trackOperation({
      authority,
      duration: 0,
      onSettled: resetResizeBaseline,
      operation: setViewport(nextViewport, { duration: 0 }),
    });
  }, [
    beginAuthority,
    getResizeBaselineSize,
    getViewport,
    layout,
    queryIdentity,
    resetResizeBaseline,
    selection,
    setViewport,
    textScale,
    trackOperation,
  ]);

  React.useEffect(() => {
    if (
      layoutKey.length === 0 ||
      handledQueryIdentity.current === queryIdentity ||
      queuedQueryIdentity.current === queryIdentity
    ) {
      return;
    }
    queuedQueryIdentity.current = queryIdentity;
    if (semanticOverlay) {
      handledQueryIdentity.current = queryIdentity;
      queuedQueryIdentity.current = null;
      return;
    }
    queueFit({
      bounds: layout.bounds,
      completion: { kind: "query", key: queryIdentity },
      duration: 0,
      maxZoom: FIT_MAX_ZOOM,
      padding: 0.08,
    });
  }, [layout.bounds, layoutKey, queryIdentity, queueFit, semanticOverlay]);

  React.useEffect(() => {
    if (semanticGeneration === "none") {
      queuedSemanticGeneration.current = null;
      return;
    }
    if (
      !semanticBounds ||
      handledSemanticGeneration.current === semanticGeneration ||
      queuedSemanticGeneration.current === semanticGeneration
    ) {
      return;
    }
    queuedSemanticGeneration.current = semanticGeneration;
    queueSemanticFit({ kind: "semantic", key: semanticGeneration });
  }, [queueSemanticFit, semanticBounds, semanticGeneration]);

  React.useEffect(() => {
    if (
      !semanticBounds ||
      fitSemanticPathsRequest === 0 ||
      fitSemanticPathsRequest === handledFitSemanticPathsRequest.current ||
      fitSemanticPathsRequest === queuedFitSemanticPathsRequest.current
    ) {
      return;
    }
    queuedFitSemanticPathsRequest.current = fitSemanticPathsRequest;
    queueSemanticFit({
      kind: "semantic-request",
      key: fitSemanticPathsRequest,
    });
  }, [fitSemanticPathsRequest, queueSemanticFit, semanticBounds]);

  React.useEffect(() => {
    if (
      focusSelectionRequest === 0 ||
      focusSelectionRequest === handledFocusSelectionRequest.current ||
      focusSelectionRequest === queuedFocusSelectionRequest.current ||
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
    queuedFocusSelectionRequest.current = focusSelectionRequest;
    focusProjectContextGraphTarget(selection);
    queueFit({
      bounds: node,
      completion: {
        kind: "focus-request",
        key: focusSelectionRequest,
        target: selection,
      },
      duration,
      maxZoom: MAX_ZOOM,
      padding: 0.8,
    });
  }, [duration, focusSelectionRequest, layout.nodes, queueFit, selection]);

  const cancelFitRequest = React.useCallback(
    (request: ProjectContextFitRequest, preserveQueuedRequest: boolean) => {
      if (preserveQueuedRequest) return;
      switch (request.completion.kind) {
        case "query":
          queuedQueryIdentity.current = null;
          break;
        case "semantic":
          queuedSemanticGeneration.current = null;
          break;
        case "semantic-request":
          queuedFitSemanticPathsRequest.current = 0;
          break;
        case "focus-request":
          queuedFocusSelectionRequest.current = 0;
          break;
        case "manual":
          break;
      }
    },
    [],
  );
  const commitFitRequest = React.useCallback(
    (request: ProjectContextFitRequest) => {
      switch (request.completion.kind) {
        case "query":
          handledQueryIdentity.current = request.completion.key;
          queuedQueryIdentity.current = null;
          setAutoFitCount((current) => current + 1);
          break;
        case "semantic":
          handledSemanticGeneration.current = request.completion.key;
          queuedSemanticGeneration.current = null;
          break;
        case "semantic-request":
          handledFitSemanticPathsRequest.current = request.completion.key;
          queuedFitSemanticPathsRequest.current = 0;
          break;
        case "focus-request":
          handledFocusSelectionRequest.current = request.completion.key;
          queuedFocusSelectionRequest.current = 0;
          focusProjectContextGraphTarget(request.completion.target);
          break;
        case "manual":
          break;
      }
      setViewportEstablished(true);
    },
    [],
  );
  useProjectContextFitSubmission({
    authoritySnapshot: viewportAuthoritySnapshot,
    currentAuthority,
    currentHumanViewportGeneration,
    fitGeneration,
    fitSuspended,
    invalidateAuthority,
    layout,
    measurement,
    measurementGeneration,
    minZoom: MIN_ZOOM,
    onCanceled: cancelFitRequest,
    onCommitted: commitFitRequest,
    pendingFit,
    queryIdentity: queryIdentityRef,
    queueFit,
    resetResizeBaseline,
    rootRef: graphRootRef,
    selection,
    semanticBounds,
    semanticGeneration: semanticGenerationRef,
    setPendingFit,
    setViewport,
    submittedFit,
    textScaleGeneration,
    trackOperation,
  });

  React.useEffect(() => {
    if (previousHoverResetKey.current === hoverResetKey) return;
    previousHoverResetKey.current = hoverResetKey;
    clearProjectContextGraphHover(graphRootRef.current);
  }, [hoverResetKey]);
  const pointerInteractions = useProjectContextPointerInteractions({
    graph,
    onSelectionChange,
    rootRef: graphRootRef,
    selection,
  });

  const fitAll = React.useCallback(() => {
    queueFit({
      bounds: layout.bounds,
      completion: { kind: "manual" },
      duration,
      maxZoom: FIT_MAX_ZOOM,
      padding: 0.08,
    });
  }, [duration, layout.bounds, queueFit]);
  const fitIsland = React.useCallback(
    (island: ProjectContextLayout["islands"][number]) => {
      queueFit({
        bounds: island.bounds,
        completion: { kind: "manual" },
        duration,
        maxZoom: FIT_MAX_ZOOM,
        padding: 0.14,
      });
    },
    [duration, queueFit],
  );
  const fitSelection = React.useCallback(() => {
    if (!selection) return;
    const nodeId =
      selection.kind === "coordinate"
        ? `coordinate:${selection.key}`
        : `edge-hub:${selection.key}`;
    const node = layout.nodes.find((candidate) => candidate.id === nodeId);
    if (!node) return;
    focusProjectContextGraphTarget(selection);
    queueFit({
      bounds: node,
      completion: { kind: "manual" },
      duration,
      maxZoom: MAX_ZOOM,
      padding: 0.8,
    });
  }, [duration, layout.nodes, queueFit, selection]);

  const contextDocumentCount = React.useMemo(
    () => projectContextDocumentCount(graph),
    [graph],
  );

  return (
    <section
      aria-describedby={
        semanticOverlay
          ? "project-context-graph-description project-context-semantic-description"
          : "project-context-graph-description"
      }
      aria-label="Project Context graph canvas"
      className="project-context-graph relative min-h-0 min-w-0 flex-1 overflow-hidden bg-background/35"
      data-auto-fit-count={autoFitCount}
      data-chrome-generation={measurement.generation}
      data-chrome-ready={measurement.ready}
      data-human-viewport-generation={
        viewportAuthoritySnapshot.humanViewportGeneration
      }
      data-semantic-freshness={
        semanticSessionOverlay ? semanticFreshness : undefined
      }
      data-semantic-overlay={semanticOverlay ? "active" : undefined}
      data-testid="project-context-graph"
      data-viewport-authority-generation={
        viewportAuthoritySnapshot.authorityGeneration
      }
      data-viewport-authority-pending={
        viewportAuthoritySnapshot.authorityPending
      }
      data-viewport-correction-count={viewportCorrectionCount}
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
        maxZoom={MAX_ZOOM}
        minZoom={MIN_ZOOM}
        nodes={elements.nodes}
        nodesConnectable={false}
        nodesDraggable={false}
        nodesFocusable={false}
        nodeTypes={NODE_TYPES}
        onEdgeClick={pointerInteractions.onEdgeClick}
        onEdgeMouseEnter={pointerInteractions.onEdgeMouseEnter}
        onEdgeMouseLeave={pointerInteractions.onEdgeMouseLeave}
        onMove={(event) => {
          if (event) continueHumanViewportGesture();
        }}
        onMoveEnd={(event) => {
          if (event) {
            endHumanViewportGesture();
          } else if (!authorityPending.current) {
            resetResizeBaseline();
          }
        }}
        onMoveStart={(event) => {
          if (event) beginHumanViewportGesture();
        }}
        onNodeClick={pointerInteractions.onNodeClick}
        onNodeMouseEnter={pointerInteractions.onNodeMouseEnter}
        onNodeMouseLeave={pointerInteractions.onNodeMouseLeave}
        onPaneClick={pointerInteractions.onPaneClick}
        onlyRenderVisibleElements={viewportEstablished}
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
        <ProjectContextCanvasHud
          contextDocumentCount={contextDocumentCount}
          externalInsets={externalInsets}
          graph={graph}
          layout={layout}
          onClearSemanticResult={
            semanticSessionOverlay ? onClearSemanticResult : undefined
          }
          onFitAll={fitAll}
          onFitIsland={fitIsland}
          onFitSelection={selection ? fitSelection : undefined}
          onFitSemanticPaths={
            semanticBounds
              ? () => queueSemanticFit({ kind: "manual" })
              : undefined
          }
          onZoomIn={() => runHumanViewportCommand(() => zoomIn({ duration }))}
          onZoomOut={() => runHumanViewportCommand(() => zoomOut({ duration }))}
          registerChromeContributor={registerChromeContributor}
          selectedLabel={selectedLabel}
          semanticFreshness={semanticFreshness}
          semanticOverlay={semanticSessionOverlay}
        />
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
    </section>
  );
}

export type { ProjectContextCanvasInsets };

/** Read-only query result graph; stable selection is owned by route state. */
export function ProjectContextGraph({
  externalCanvasInsets,
  fitSemanticPathsRequest = 0,
  fitSuspended = false,
  focusSelectionRequest = 0,
  onClearSemanticResult,
  onSelectionChange,
  result,
  selection,
  semanticFreshness = "snapshot",
  semanticOverlay = null,
  semanticSessionOverlay = semanticOverlay,
}: {
  externalCanvasInsets?: Partial<ProjectContextCanvasInsets>;
  fitSemanticPathsRequest?: number;
  fitSuspended?: boolean;
  focusSelectionRequest?: number;
  onClearSemanticResult?: () => void;
  onSelectionChange: (
    selection: ProjectContextGraphTarget | null,
    options?: { replace?: boolean },
  ) => void;
  result: ProjectContextQueryResult;
  selection: ProjectContextGraphTarget | null;
  semanticFreshness?: ProjectContextSemanticFreshness;
  semanticOverlay?: ProjectContextSemanticOverlay | null;
  semanticSessionOverlay?: ProjectContextSemanticOverlay | null;
}) {
  const textScale = useProjectContextTextScale();
  const externalBottom = externalCanvasInsets?.bottom;
  const externalLeft = externalCanvasInsets?.left;
  const externalRight = externalCanvasInsets?.right;
  const externalTop = externalCanvasInsets?.top;
  const externalInsets = React.useMemo(
    () =>
      mergeProjectContextCanvasInsets(EMPTY_PROJECT_CONTEXT_CANVAS_INSETS, {
        bottom: externalBottom,
        left: externalLeft,
        right: externalRight,
        top: externalTop,
      }),
    [externalBottom, externalLeft, externalRight, externalTop],
  );
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

  return (
    <ReactFlowProvider>
      <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        <ProjectContextGraphInner
          elements={elements}
          externalInsets={externalInsets}
          fitSemanticPathsRequest={fitSemanticPathsRequest}
          fitSuspended={fitSuspended}
          focusSelectionRequest={focusSelectionRequest}
          graph={graph}
          layout={layout}
          onClearSemanticResult={onClearSemanticResult}
          onSelectionChange={onSelectionChange}
          queryIdentity={topology.queryIdentity}
          selection={selection}
          semanticFreshness={semanticFreshness}
          semanticOverlay={visibleSemanticOverlay}
          semanticSessionOverlay={semanticSessionOverlay}
          textScale={textScale}
        />
      </section>
    </ReactFlowProvider>
  );
}
