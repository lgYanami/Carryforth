import type { Edge, Node } from "@xyflow/react";

import type {
  ProjectContextGraphCoordinate,
  ProjectContextGraphHub,
  ProjectContextGraphModel,
} from "@/features/project-context/graph";
import type {
  ProjectContextIslandLayout,
  ProjectContextLayout,
} from "@/features/project-context/layout";
import type { ProjectContextSemanticOverlay } from "@/features/project-context/semanticOverlay";

export type ProjectContextGraphTarget =
  | { kind: "coordinate"; key: string }
  | { kind: "edge"; key: string };

export type ProjectContextEmphasis = "normal" | "active" | "dimmed";
export type ProjectContextSemanticEmphasis =
  | "none"
  | "outside"
  | "member"
  | "route";

export type ProjectContextIslandNodeData = {
  kind: "island";
  island: ProjectContextIslandLayout;
  hue: number;
};

export type ProjectContextCoordinateNodeData = {
  kind: "coordinate";
  coordinate: ProjectContextGraphCoordinate;
  emphasis: ProjectContextEmphasis;
  islandIndex: number;
  hue: number;
  queryAnchor: boolean;
  semanticEmphasis: ProjectContextSemanticEmphasis;
  semanticRoot: boolean;
  semanticTerminal: boolean;
  selected: boolean;
};

export type ProjectContextHubNodeData = {
  kind: "hub";
  hub: ProjectContextGraphHub;
  emphasis: ProjectContextEmphasis;
  islandIndex: number;
  hue: number;
  semanticEmphasis: ProjectContextSemanticEmphasis;
  semanticRoot: boolean;
  selected: boolean;
};

export type ProjectContextSpokeData = {
  kind: "spoke";
  edgeKey: string;
  coordinateKey: string;
  emphasis: ProjectContextEmphasis;
  islandIndex: number;
  hue: number;
  semanticEmphasis: ProjectContextSemanticEmphasis;
};

export type ProjectContextIslandFlowNode = Node<
  ProjectContextIslandNodeData,
  "contextIsland"
>;
export type ProjectContextCoordinateFlowNode = Node<
  ProjectContextCoordinateNodeData,
  "contextCoordinate"
>;
export type ProjectContextHubFlowNode = Node<
  ProjectContextHubNodeData,
  "contextHub"
>;
export type ProjectContextFlowNode =
  | ProjectContextIslandFlowNode
  | ProjectContextCoordinateFlowNode
  | ProjectContextHubFlowNode;
export type ProjectContextFlowEdge = Edge<
  ProjectContextSpokeData,
  "contextSpoke"
>;

export type ProjectContextFlowElements = {
  nodes: ProjectContextFlowNode[];
  edges: ProjectContextFlowEdge[];
};

const ISLAND_HUES = [267, 196, 151, 32, 338, 224, 12, 178];
const ISLAND_CYCLE_OFFSET = 23;

/** Stable, presentation-only hue for the current deterministic Island order. */
export function projectContextIslandHue(index: number): number {
  const normalized = Math.max(index, 1) - 1;
  const cycle = Math.floor(normalized / ISLAND_HUES.length);
  return (
    (ISLAND_HUES[normalized % ISLAND_HUES.length] +
      cycle * ISLAND_CYCLE_OFFSET) %
    360
  );
}

function emphasis(active: boolean, target: ProjectContextGraphTarget | null) {
  if (!target) return "normal";
  return active ? "active" : "dimmed";
}

function activeKeys(
  graph: ProjectContextGraphModel,
  target: ProjectContextGraphTarget | null,
) {
  const coordinates = new Set<string>();
  const hubs = new Set<string>();
  if (!target) return { coordinates, hubs };
  if (target.kind === "edge") {
    hubs.add(target.key);
    for (const coordinateKey of graph.hubs.find(
      (hub) => hub.edgeKey === target.key,
    )?.coordinateKeys ?? []) {
      coordinates.add(coordinateKey);
    }
    return { coordinates, hubs };
  }
  coordinates.add(target.key);
  for (const hub of graph.hubs) {
    if (hub.coordinateKeys.includes(target.key)) hubs.add(hub.edgeKey);
  }
  return { coordinates, hubs };
}

function coordinateSemanticPresentation(
  coordinateKey: string,
  overlay: ProjectContextSemanticOverlay | null,
): {
  emphasis: ProjectContextSemanticEmphasis;
  root: boolean;
  terminal: boolean;
} {
  if (!overlay || overlay.boundsTargetIds.length === 0) {
    return { emphasis: "none", root: false, terminal: false };
  }
  const root = overlay.rootCoordinateKeys.has(coordinateKey);
  const terminal = overlay.terminalCoordinateKeys.has(coordinateKey);
  if (root || terminal || overlay.routeCoordinateKeys.has(coordinateKey)) {
    return { emphasis: "route", root, terminal };
  }
  if (overlay.memberCoordinateKeys.has(coordinateKey)) {
    return { emphasis: "member", root, terminal };
  }
  return { emphasis: "outside", root, terminal };
}

function hubSemanticPresentation(
  edgeKey: string,
  overlay: ProjectContextSemanticOverlay | null,
): {
  emphasis: ProjectContextSemanticEmphasis;
  root: boolean;
} {
  if (!overlay || overlay.boundsTargetIds.length === 0) {
    return { emphasis: "none", root: false };
  }
  const root = overlay.rootEdgeKeys.has(edgeKey);
  if (overlay.edgeKeys.has(edgeKey)) return { emphasis: "route", root };
  if (root) return { emphasis: "member", root };
  return { emphasis: "outside", root };
}

/**
 * Adapt canonical graph and layout data to immutable React Flow elements.
 * A selected Edge highlights exactly its Hub, Spokes, and Coordinate set.
 */
export function buildProjectContextFlowElements(
  graph: ProjectContextGraphModel,
  layout: ProjectContextLayout,
  target: ProjectContextGraphTarget | null,
  semanticOverlay: ProjectContextSemanticOverlay | null = null,
): ProjectContextFlowElements {
  const coordinateById = new Map(
    graph.coordinates.map((coordinate) => [coordinate.id, coordinate]),
  );
  const hubById = new Map(graph.hubs.map((hub) => [hub.id, hub]));
  const active = activeKeys(graph, target);
  const layoutNodeById = new Map(layout.nodes.map((node) => [node.id, node]));
  const layoutSpokeById = new Map(
    layout.spokes.map((spoke) => [spoke.id, spoke]),
  );
  const nodes: ProjectContextFlowNode[] = layout.islands.map((island) => ({
    id: `context-island:${island.stableKey}`,
    type: "contextIsland",
    position: { x: island.bounds.x, y: island.bounds.y },
    style: { width: island.bounds.width, height: island.bounds.height },
    data: {
      kind: "island",
      island,
      hue: projectContextIslandHue(island.index),
    },
    draggable: false,
    selectable: false,
    focusable: false,
    zIndex: -10,
    className: "pointer-events-none",
  }));
  const anchorKeys = new Set(graph.anchorCoordinateKeys);

  for (const layoutNode of layout.nodes) {
    const hue = projectContextIslandHue(layoutNode.islandIndex);
    if (layoutNode.kind === "coordinate") {
      const coordinate = coordinateById.get(layoutNode.id);
      if (!coordinate) continue;
      const semantic = coordinateSemanticPresentation(
        coordinate.coordinateKey,
        semanticOverlay,
      );
      nodes.push({
        id: coordinate.id,
        type: "contextCoordinate",
        position: { x: layoutNode.x, y: layoutNode.y },
        style: { width: layoutNode.width, height: layoutNode.height },
        data: {
          kind: "coordinate",
          coordinate,
          emphasis: emphasis(
            active.coordinates.has(coordinate.coordinateKey),
            target,
          ),
          islandIndex: layoutNode.islandIndex,
          hue,
          queryAnchor: anchorKeys.has(coordinate.coordinateKey),
          semanticEmphasis: semantic.emphasis,
          semanticRoot: semantic.root,
          semanticTerminal: semantic.terminal,
          selected:
            target?.kind === "coordinate" &&
            target.key === coordinate.coordinateKey,
        },
        draggable: false,
        selectable: false,
        focusable: false,
        zIndex: 2,
      });
      continue;
    }

    const hub = hubById.get(layoutNode.id);
    if (!hub) continue;
    const semantic = hubSemanticPresentation(hub.edgeKey, semanticOverlay);
    nodes.push({
      id: hub.id,
      type: "contextHub",
      position: { x: layoutNode.x, y: layoutNode.y },
      style: { width: layoutNode.width, height: layoutNode.height },
      data: {
        kind: "hub",
        hub,
        emphasis: emphasis(active.hubs.has(hub.edgeKey), target),
        islandIndex: layoutNode.islandIndex,
        hue,
        semanticEmphasis: semantic.emphasis,
        semanticRoot: semantic.root,
        selected: target?.kind === "edge" && target.key === hub.edgeKey,
      },
      draggable: false,
      selectable: false,
      focusable: false,
      zIndex: 3,
    });
  }

  const edges: ProjectContextFlowEdge[] = graph.spokes.flatMap((spoke) => {
    const hubLayout = layoutNodeById.get(spoke.sourceId);
    const spokeLayout = layoutSpokeById.get(spoke.id);
    if (!hubLayout || !spokeLayout) return [];
    return [
      {
        id: spoke.id,
        type: "contextSpoke",
        source: spoke.sourceId,
        target: spoke.targetId,
        sourceHandle: spokeLayout.sourceHandle,
        targetHandle: spokeLayout.targetHandle,
        data: {
          kind: "spoke",
          edgeKey: spoke.edgeKey,
          coordinateKey: spoke.coordinateKey,
          emphasis: emphasis(
            target?.kind === "edge"
              ? active.hubs.has(spoke.edgeKey)
              : active.coordinates.has(spoke.coordinateKey),
            target,
          ),
          islandIndex: hubLayout.islandIndex,
          hue: projectContextIslandHue(hubLayout.islandIndex),
          semanticEmphasis:
            semanticOverlay && semanticOverlay.boundsTargetIds.length > 0
              ? semanticOverlay.edgeKeys.has(spoke.edgeKey)
                ? "member"
                : "outside"
              : "none",
        },
        deletable: false,
        domAttributes: { "aria-hidden": true },
        reconnectable: false,
        selectable: false,
        focusable: false,
        interactionWidth: 28,
        zIndex: 1,
      },
    ];
  });

  return { nodes, edges };
}
