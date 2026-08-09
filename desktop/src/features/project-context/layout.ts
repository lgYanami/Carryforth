import type {
  ProjectContextGraphIsland,
  ProjectContextGraphModel,
} from "@/features/project-context/graph";
import {
  layoutRadialComponent,
  type RadialLayoutLinkInput,
  type RadialLayoutNodeInput,
} from "@/features/project-context/radialLayout";

export type ProjectContextPort = "top" | "right" | "bottom" | "left";

export type ProjectContextLayoutNode = {
  id: string;
  kind: "coordinate" | "hub";
  islandKey: string;
  islandIndex: number;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type ProjectContextLayoutSpoke = {
  id: string;
  sourceHandle: ProjectContextPort;
  targetHandle: ProjectContextPort;
};

export type ProjectContextBounds = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type ProjectContextIslandLayout = ProjectContextGraphIsland & {
  bounds: ProjectContextBounds;
};

export type ProjectContextLayout = {
  nodes: ProjectContextLayoutNode[];
  spokes: ProjectContextLayoutSpoke[];
  islands: ProjectContextIslandLayout[];
  bounds: ProjectContextBounds;
};

export type ProjectContextLayoutTopology = {
  descriptor: string;
  queryIdentity: string;
  queryMode: ProjectContextGraphModel["queryMode"];
  isAllContext: boolean;
  anchorNodeIds: string[];
  nodes: Array<{ id: string; kind: ProjectContextLayoutNode["kind"] }>;
  spokes: Array<{
    id: string;
    edgeKey: string;
    sourceId: string;
    targetId: string;
  }>;
  islands: Array<{
    stableKey: string;
    index: number;
    edgeKeys: string[];
    nodeIds: string[];
  }>;
};

export type ProjectContextLayoutGeometry = {
  nodes: ProjectContextLayoutNode[];
  spokes: ProjectContextLayoutSpoke[];
  islands: Array<{
    stableKey: string;
    index: number;
    bounds: ProjectContextBounds;
  }>;
  bounds: ProjectContextBounds;
};

const BASE_COORDINATE_WIDTH = 224;
const BASE_COORDINATE_HEIGHT = 120;
const BASE_HUB_SIZE = 76;
const BASE_LAYER_GAP = 116;
const BASE_ROW_GAP = 52;
const BASE_ISLAND_PADDING_X = 72;
const BASE_ISLAND_PADDING_TOP = 92;
const BASE_ISLAND_PADDING_BOTTOM = 64;
const BASE_MIN_ISLAND_WIDTH = 420;
const BASE_ISLAND_GAP = 128;
const BASE_OUTER_PADDING = 64;

function compareText(left: string, right: string) {
  return left.localeCompare(right, "en");
}

function dimensions(kind: ProjectContextLayoutNode["kind"]) {
  if (kind === "hub") {
    return { width: BASE_HUB_SIZE, height: BASE_HUB_SIZE };
  }
  return {
    width: BASE_COORDINATE_WIDTH,
    height: BASE_COORDINATE_HEIGHT,
  };
}

function nodeKind(id: string): ProjectContextLayoutNode["kind"] {
  return id.startsWith("edge-hub:") ? "hub" : "coordinate";
}

function sortedUnique(values: readonly string[]) {
  return [...new Set(values)].sort(compareText);
}

/**
 * Build the exact presentation topology used by the layout solver. Titles,
 * summaries, lifecycle state, and Context Document membership are excluded so
 * metadata-only changes cannot move nodes.
 */
export function buildProjectContextLayoutTopology(
  graph: ProjectContextGraphModel,
): ProjectContextLayoutTopology {
  const nodes = [
    ...graph.coordinates.map((coordinate) => ({
      id: coordinate.id,
      kind: "coordinate" as const,
    })),
    ...graph.hubs.map((hub) => ({ id: hub.id, kind: "hub" as const })),
  ].sort((left, right) => compareText(left.id, right.id));
  const spokes = graph.spokes
    .map((spoke) => ({
      id: spoke.id,
      edgeKey: spoke.edgeKey,
      sourceId: spoke.sourceId,
      targetId: spoke.targetId,
    }))
    .sort((left, right) => compareText(left.id, right.id));
  const anchorNodeIds = sortedUnique(
    (graph.anchorCoordinateKeys ?? []).map((key) => `coordinate:${key}`),
  );
  const islands = graph.islands
    .map((island) => {
      const edgeKeys = sortedUnique(island.edgeKeys);
      const edgeKeySet = new Set(edgeKeys);
      return {
        stableKey: island.stableKey,
        index: island.index,
        edgeKeys,
        nodeIds: sortedUnique(
          spokes
            .filter((spoke) => edgeKeySet.has(spoke.edgeKey))
            .flatMap((spoke) => [spoke.sourceId, spoke.targetId]),
        ),
      };
    })
    .sort((left, right) => compareText(left.stableKey, right.stableKey));
  const queryMode = graph.queryMode ?? "contains_all";
  const isAllContext = graph.isAllContext !== false;
  const canonical = {
    queryMode,
    isAllContext,
    anchorNodeIds,
    nodes,
    spokes,
    islands,
  };
  return {
    ...canonical,
    descriptor: JSON.stringify(canonical),
    queryIdentity: `${queryMode}:${anchorNodeIds.join("|")}`,
  };
}

function adjacency(
  nodeIds: readonly string[],
  links: readonly RadialLayoutLinkInput[],
) {
  const included = new Set(nodeIds);
  const result = new Map<string, Set<string>>();
  for (const id of nodeIds) result.set(id, new Set());
  for (const link of links) {
    if (!included.has(link.sourceId) || !included.has(link.targetId)) continue;
    result.get(link.sourceId)?.add(link.targetId);
    result.get(link.targetId)?.add(link.sourceId);
  }
  return result;
}

function farthestNode(start: string, graph: ReadonlyMap<string, Set<string>>) {
  const distance = new Map([[start, 0]]);
  const parent = new Map<string, string | undefined>([[start, undefined]]);
  const queue = [start];
  let queueIndex = 0;
  while (queueIndex < queue.length) {
    const current = queue[queueIndex];
    queueIndex += 1;
    for (const neighbor of [...(graph.get(current) ?? [])].sort(compareText)) {
      if (distance.has(neighbor)) continue;
      distance.set(neighbor, (distance.get(current) ?? 0) + 1);
      parent.set(neighbor, current);
      queue.push(neighbor);
    }
  }
  const id = [...distance.keys()].sort(
    (left, right) =>
      (distance.get(right) ?? 0) - (distance.get(left) ?? 0) ||
      compareText(left, right),
  )[0];
  return { id, parent };
}

function graphCenter(
  nodeIds: readonly string[],
  graph: ReadonlyMap<string, Set<string>>,
) {
  const start = [...nodeIds].sort(compareText)[0];
  if (!start) return undefined;
  const endpoint = farthestNode(start, graph).id;
  if (!endpoint) return start;
  const sweep = farthestNode(endpoint, graph);
  if (!sweep.id) return endpoint;
  const path = [sweep.id];
  while (path[path.length - 1] !== endpoint) {
    const next = sweep.parent.get(path[path.length - 1]);
    if (!next) break;
    path.push(next);
  }
  const centerIndex = (path.length - 1) / 2;
  if (Number.isInteger(centerIndex)) return path[centerIndex];
  const candidates = [
    path[Math.floor(centerIndex)],
    path[Math.ceil(centerIndex)],
  ];
  return candidates.sort((left, right) => {
    const degree = (graph.get(right)?.size ?? 0) - (graph.get(left)?.size ?? 0);
    if (degree !== 0) return degree;
    const hub =
      Number(nodeKind(right) === "hub") - Number(nodeKind(left) === "hub");
    return hub || compareText(left, right);
  })[0];
}

function layeredNodeIds(
  nodeIds: readonly string[],
  graph: ReadonlyMap<string, Set<string>>,
  root: string,
) {
  const distance = new Map([[root, 0]]);
  const queue = [root];
  let queueIndex = 0;
  while (queueIndex < queue.length) {
    const current = queue[queueIndex];
    queueIndex += 1;
    for (const neighbor of [...(graph.get(current) ?? [])].sort(compareText)) {
      if (distance.has(neighbor)) continue;
      distance.set(neighbor, (distance.get(current) ?? 0) + 1);
      queue.push(neighbor);
    }
  }
  const maxDistance = Math.max(0, ...distance.values());
  const layers = Array.from({ length: maxDistance + 1 }, () => [] as string[]);
  for (const id of [...nodeIds].sort(compareText)) {
    layers[distance.get(id) ?? 0].push(id);
  }
  return layers;
}

function fallbackLayeredComponent(
  stableKey: string,
  islandIndex: number,
  nodeIds: readonly string[],
  links: readonly RadialLayoutLinkInput[],
) {
  const graph = adjacency(nodeIds, links);
  const root = graphCenter(nodeIds, graph) ?? nodeIds[0];
  const layers = root ? layeredNodeIds(nodeIds, graph, root) : [];
  const layerWidths = layers.map((layer) =>
    Math.max(0, ...layer.map((id) => dimensions(nodeKind(id)).width)),
  );
  const layerHeights = layers.map((layer) =>
    layer.reduce(
      (height, id, index) =>
        height +
        dimensions(nodeKind(id)).height +
        (index === 0 ? 0 : BASE_ROW_GAP),
      0,
    ),
  );
  const innerWidth =
    layerWidths.reduce((sum, width) => sum + width, 0) +
    Math.max(0, layers.length - 1) * BASE_LAYER_GAP;
  const innerHeight = Math.max(0, ...layerHeights);
  const width = Math.max(
    BASE_MIN_ISLAND_WIDTH,
    innerWidth + BASE_ISLAND_PADDING_X * 2,
  );
  const height =
    innerHeight + BASE_ISLAND_PADDING_TOP + BASE_ISLAND_PADDING_BOTTOM;
  const nodes: ProjectContextLayoutNode[] = [];
  let layerX =
    BASE_ISLAND_PADDING_X +
    (width - BASE_ISLAND_PADDING_X * 2 - innerWidth) / 2;
  layers.forEach((layer, layerIndex) => {
    let nodeY =
      BASE_ISLAND_PADDING_TOP + (innerHeight - layerHeights[layerIndex]) / 2;
    for (const id of layer) {
      const kind = nodeKind(id);
      const size = dimensions(kind);
      nodes.push({
        id,
        kind,
        islandKey: stableKey,
        islandIndex,
        x: layerX + (layerWidths[layerIndex] - size.width) / 2,
        y: nodeY,
        ...size,
      });
      nodeY += size.height + BASE_ROW_GAP;
    }
    layerX += layerWidths[layerIndex] + BASE_LAYER_GAP;
  });
  return { nodes, width, height };
}

function radialComponent(
  stableKey: string,
  islandIndex: number,
  nodeIds: readonly string[],
  links: readonly RadialLayoutLinkInput[],
  centerIds: readonly string[],
  virtualCenter: boolean,
  focused: boolean,
) {
  const radialNodes: RadialLayoutNodeInput[] = [...nodeIds]
    .sort(compareText)
    .map((id) => ({ id, kind: nodeKind(id), ...dimensions(nodeKind(id)) }));
  const radial = layoutRadialComponent({
    stableKey,
    nodes: radialNodes,
    links,
    centerIds: [...centerIds],
    virtualCenter,
  });
  if (!radial) {
    return fallbackLayeredComponent(stableKey, islandIndex, nodeIds, links);
  }
  const positionById = new Map(
    radial.positions.map((position) => [position.id, position]),
  );
  const raw = radialNodes.map((node) => {
    const position = positionById.get(node.id);
    return {
      id: node.id,
      kind: node.kind,
      centerX: position?.centerX ?? 0,
      centerY: position?.centerY ?? 0,
      width: node.width,
      height: node.height,
      depth: position?.depth ?? 0,
      band: position?.band ?? 0,
    };
  });
  const minX = Math.min(...raw.map((node) => node.centerX - node.width / 2));
  const maxX = Math.max(...raw.map((node) => node.centerX + node.width / 2));
  const minY = Math.min(...raw.map((node) => node.centerY - node.height / 2));
  const maxY = Math.max(...raw.map((node) => node.centerY + node.height / 2));
  const paddingX = focused ? BASE_OUTER_PADDING : BASE_ISLAND_PADDING_X;
  const paddingTop = focused ? BASE_OUTER_PADDING : BASE_ISLAND_PADDING_TOP;
  const paddingBottom = focused
    ? BASE_OUTER_PADDING
    : BASE_ISLAND_PADDING_BOTTOM;
  const contentWidth = maxX - minX;
  const width = Math.max(
    focused ? 0 : BASE_MIN_ISLAND_WIDTH,
    contentWidth + paddingX * 2,
  );
  const leftOffset =
    paddingX + (width - paddingX * 2 - contentWidth) / 2 - minX;
  const topOffset = paddingTop - minY;
  const anchorSet = new Set(centerIds);
  const nodes = raw
    .sort(
      (left, right) =>
        Number(anchorSet.has(right.id)) - Number(anchorSet.has(left.id)) ||
        left.depth - right.depth ||
        left.band - right.band ||
        Number(left.kind === "hub") - Number(right.kind === "hub") ||
        compareText(left.id, right.id),
    )
    .map(({ centerX, centerY, depth: _depth, band: _band, ...node }) => ({
      ...node,
      islandKey: stableKey,
      islandIndex,
      x: centerX - node.width / 2 + leftOffset,
      y: centerY - node.height / 2 + topOffset,
    }));
  return { nodes, width, height: maxY - minY + paddingTop + paddingBottom };
}

function componentLinks(
  topology: ProjectContextLayoutTopology,
  nodeIds: readonly string[],
) {
  const included = new Set(nodeIds);
  return topology.spokes
    .filter(
      (spoke) => included.has(spoke.sourceId) && included.has(spoke.targetId),
    )
    .map((spoke) => ({ sourceId: spoke.sourceId, targetId: spoke.targetId }));
}

function focusedGeometry(topology: ProjectContextLayoutTopology) {
  if (topology.nodes.length === 0) {
    return { nodes: [], width: 0, height: 0 };
  }
  const nodeIds = topology.nodes.map((node) => node.id);
  const links = componentLinks(topology, nodeIds);
  const hubIds = topology.nodes
    .filter((node) => node.kind === "hub")
    .map((node) => node.id);
  let centerIds = topology.anchorNodeIds;
  let virtualCenter = centerIds.length !== 1;
  if (topology.queryMode === "exact" && hubIds.length === 1) {
    centerIds = hubIds;
    virtualCenter = false;
  } else if (topology.queryMode === "incident" && centerIds.length === 1) {
    virtualCenter = false;
  } else if (topology.queryMode === "contains_all" && hubIds.length > 0) {
    virtualCenter = true;
  } else if (hubIds.length === 0 && centerIds.length === 1) {
    virtualCenter = false;
  }
  return radialComponent(
    `query:${topology.queryMode}:${topology.anchorNodeIds.join("|")}`,
    1,
    nodeIds,
    links,
    centerIds,
    virtualCenter,
    true,
  );
}

/**
 * Compute canonical scale-1 geometry for one exact layout topology. Callers
 * may cache this value while separately rehydrating current graph metadata.
 */
export function layoutProjectContextGeometry(
  topology: ProjectContextLayoutTopology,
): ProjectContextLayoutGeometry {
  if (!topology.isAllContext) {
    const focused = focusedGeometry(topology);
    const nodes = focused.nodes;
    const nodeById = new Map(nodes.map((node) => [node.id, node]));
    return {
      nodes,
      spokes: topology.spokes.map((spoke) => ({
        id: spoke.id,
        sourceHandle: projectContextPortToward(
          nodeById.get(spoke.sourceId),
          nodeById.get(spoke.targetId),
        ),
        targetHandle: projectContextPortToward(
          nodeById.get(spoke.targetId),
          nodeById.get(spoke.sourceId),
        ),
      })),
      islands: [],
      bounds: { x: 0, y: 0, width: focused.width, height: focused.height },
    };
  }
  if (topology.islands.length === 0) {
    return {
      nodes: [],
      spokes: [],
      islands: [],
      bounds: { x: 0, y: 0, width: 0, height: 0 },
    };
  }

  const components = topology.islands.map((island) => {
    const links = componentLinks(topology, island.nodeIds);
    const graph = adjacency(island.nodeIds, links);
    const center = graphCenter(island.nodeIds, graph);
    return {
      island,
      ...radialComponent(
        island.stableKey,
        island.index,
        island.nodeIds,
        links,
        center ? [center] : [],
        false,
        false,
      ),
    };
  });
  const columnCount = Math.ceil(Math.sqrt(components.length));
  const rowCount = Math.ceil(components.length / columnCount);
  const columnWidths = Array.from({ length: columnCount }, () => 0);
  const rowHeights = Array.from({ length: rowCount }, () => 0);
  components.forEach((component, index) => {
    const column = index % columnCount;
    const row = Math.floor(index / columnCount);
    columnWidths[column] = Math.max(columnWidths[column], component.width);
    rowHeights[row] = Math.max(rowHeights[row], component.height);
  });
  const columnX: number[] = [];
  const rowY: number[] = [];
  let next = BASE_OUTER_PADDING;
  for (const width of columnWidths) {
    columnX.push(next);
    next += width + BASE_ISLAND_GAP;
  }
  next = BASE_OUTER_PADDING;
  for (const height of rowHeights) {
    rowY.push(next);
    next += height + BASE_ISLAND_GAP;
  }
  const nodes: ProjectContextLayoutNode[] = [];
  const islands: ProjectContextLayoutGeometry["islands"] = [];
  components.forEach((component, index) => {
    const column = index % columnCount;
    const row = Math.floor(index / columnCount);
    const x = columnX[column] + (columnWidths[column] - component.width) / 2;
    const y = rowY[row] + (rowHeights[row] - component.height) / 2;
    islands.push({
      stableKey: component.island.stableKey,
      index: component.island.index,
      bounds: { x, y, width: component.width, height: component.height },
    });
    nodes.push(
      ...component.nodes.map((node) => ({
        ...node,
        x: node.x + x,
        y: node.y + y,
      })),
    );
  });
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const spokes = topology.spokes.map((spoke) => ({
    id: spoke.id,
    sourceHandle: projectContextPortToward(
      nodeById.get(spoke.sourceId),
      nodeById.get(spoke.targetId),
    ),
    targetHandle: projectContextPortToward(
      nodeById.get(spoke.targetId),
      nodeById.get(spoke.sourceId),
    ),
  }));
  const width =
    BASE_OUTER_PADDING * 2 +
    columnWidths.reduce((sum, value) => sum + value, 0) +
    Math.max(0, columnCount - 1) * BASE_ISLAND_GAP;
  const height =
    BASE_OUTER_PADDING * 2 +
    rowHeights.reduce((sum, value) => sum + value, 0) +
    Math.max(0, rowCount - 1) * BASE_ISLAND_GAP;
  return { nodes, spokes, islands, bounds: { x: 0, y: 0, width, height } };
}

function scaleValue(value: number, scale: number) {
  const result = value * scale;
  return Object.is(result, -0) ? 0 : result;
}

function scaleBounds(bounds: ProjectContextBounds, scale: number) {
  return {
    x: scaleValue(bounds.x, scale),
    y: scaleValue(bounds.y, scale),
    width: scaleValue(bounds.width, scale),
    height: scaleValue(bounds.height, scale),
  };
}

/**
 * Rehydrate current Island facts and apply text scale without rerunning the
 * topology solver. Project Context data remains owned by the graph model.
 */
export function materializeProjectContextLayout(
  geometry: ProjectContextLayoutGeometry,
  graph: ProjectContextGraphModel,
  requestedScale = 1,
): ProjectContextLayout {
  const scale = Number.isFinite(requestedScale)
    ? Math.min(Math.max(requestedScale, 0.75), 1.5)
    : 1;
  const islandByKey = new Map(
    graph.islands.map((island) => [island.stableKey, island]),
  );
  return {
    nodes: geometry.nodes.map((node) => ({
      ...node,
      x: scaleValue(node.x, scale),
      y: scaleValue(node.y, scale),
      width: scaleValue(node.width, scale),
      height: scaleValue(node.height, scale),
    })),
    spokes: geometry.spokes,
    islands: geometry.islands.flatMap((island) => {
      const facts = islandByKey.get(island.stableKey);
      return facts
        ? [{ ...facts, bounds: scaleBounds(island.bounds, scale) }]
        : [];
    }),
    bounds: scaleBounds(geometry.bounds, scale),
  };
}

/** True when two layout bounds overlap rather than merely touch. */
export function projectContextBoundsOverlap(
  left: ProjectContextBounds,
  right: ProjectContextBounds,
): boolean {
  return (
    left.x < right.x + right.width &&
    left.x + left.width > right.x &&
    left.y < right.y + right.height &&
    left.y + left.height > right.y
  );
}

/** Select the midpoint Handle on the first rectangle facing the second one. */
export function projectContextPortToward(
  source: ProjectContextLayoutNode | undefined,
  target: ProjectContextLayoutNode | undefined,
): ProjectContextPort {
  if (!source || !target) return "right";
  const deltaX = target.x + target.width / 2 - (source.x + source.width / 2);
  const deltaY = target.y + target.height / 2 - (source.y + source.height / 2);
  const normalizedX = Math.abs(deltaX) / Math.max(1, source.width / 2);
  const normalizedY = Math.abs(deltaY) / Math.max(1, source.height / 2);
  if (normalizedX >= normalizedY) return deltaX >= 0 ? "right" : "left";
  return deltaY >= 0 ? "bottom" : "top";
}

/**
 * Convenience wrapper for tests and non-React callers. Desktop rendering uses
 * the split topology/geometry/materialization functions to avoid solving on
 * metadata-only updates.
 */
export function layoutProjectContextGraph(
  graph: ProjectContextGraphModel,
  requestedScale = 1,
): ProjectContextLayout {
  const topology = buildProjectContextLayoutTopology(graph);
  return materializeProjectContextLayout(
    layoutProjectContextGeometry(topology),
    graph,
    requestedScale,
  );
}
