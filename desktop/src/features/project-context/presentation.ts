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

export type ProjectContextGraphTarget =
  | { kind: "coordinate"; key: string }
  | { kind: "edge"; key: string };

export type ProjectContextEmphasis = "normal" | "active" | "dimmed";

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
};

export type ProjectContextHubNodeData = {
  kind: "hub";
  hub: ProjectContextGraphHub;
  emphasis: ProjectContextEmphasis;
  islandIndex: number;
  hue: number;
};

export type ProjectContextSpokeData = {
  kind: "spoke";
  edgeKey: string;
  coordinateKey: string;
  emphasis: ProjectContextEmphasis;
  islandIndex: number;
  hue: number;
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

/** Stable, presentation-only hue for the current deterministic Island order. */
export function projectContextIslandHue(index: number): number {
  return ISLAND_HUES[(Math.max(index, 1) - 1) % ISLAND_HUES.length];
}

function emphasis(active: boolean, target: ProjectContextGraphTarget | null) {
  if (!target) return "normal";
  return active ? "active" : "dimmed";
}

function coordinateIsActive(
  coordinateKey: string,
  hubsByKey: Map<string, ProjectContextGraphHub>,
  target: ProjectContextGraphTarget | null,
) {
  if (!target) return false;
  if (target.kind === "coordinate") return target.key === coordinateKey;
  return (
    hubsByKey.get(target.key)?.coordinateKeys.includes(coordinateKey) ?? false
  );
}

function hubIsActive(
  hub: ProjectContextGraphHub,
  target: ProjectContextGraphTarget | null,
) {
  if (!target) return false;
  if (target.kind === "edge") return target.key === hub.edgeKey;
  return hub.coordinateKeys.includes(target.key);
}

function spokeIsActive(
  edgeKey: string,
  coordinateKey: string,
  target: ProjectContextGraphTarget | null,
) {
  if (!target) return false;
  return target.kind === "edge"
    ? target.key === edgeKey
    : target.key === coordinateKey;
}

/**
 * Adapt canonical graph and layout data to immutable React Flow elements.
 * A selected Edge highlights exactly its Hub, Spokes, and Coordinate set.
 */
export function buildProjectContextFlowElements(
  graph: ProjectContextGraphModel,
  layout: ProjectContextLayout,
  target: ProjectContextGraphTarget | null,
): ProjectContextFlowElements {
  const coordinateById = new Map(
    graph.coordinates.map((coordinate) => [coordinate.id, coordinate]),
  );
  const hubById = new Map(graph.hubs.map((hub) => [hub.id, hub]));
  const hubsByKey = new Map(graph.hubs.map((hub) => [hub.edgeKey, hub]));
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
      nodes.push({
        id: coordinate.id,
        type: "contextCoordinate",
        position: { x: layoutNode.x, y: layoutNode.y },
        style: { width: layoutNode.width, height: layoutNode.height },
        data: {
          kind: "coordinate",
          coordinate,
          emphasis: emphasis(
            coordinateIsActive(coordinate.coordinateKey, hubsByKey, target),
            target,
          ),
          islandIndex: layoutNode.islandIndex,
          hue,
          queryAnchor: anchorKeys.has(coordinate.coordinateKey),
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
    nodes.push({
      id: hub.id,
      type: "contextHub",
      position: { x: layoutNode.x, y: layoutNode.y },
      style: { width: layoutNode.width, height: layoutNode.height },
      data: {
        kind: "hub",
        hub,
        emphasis: emphasis(hubIsActive(hub, target), target),
        islandIndex: layoutNode.islandIndex,
        hue,
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
            spokeIsActive(spoke.edgeKey, spoke.coordinateKey, target),
            target,
          ),
          islandIndex: hubLayout.islandIndex,
          hue: projectContextIslandHue(hubLayout.islandIndex),
        },
        ariaLabel: `Context Edge ${spoke.edgeKey} incidence`,
        deletable: false,
        reconnectable: false,
        selectable: false,
        focusable: true,
        interactionWidth: 28,
        zIndex: 1,
      },
    ];
  });

  return { nodes, edges };
}
