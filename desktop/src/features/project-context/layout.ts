import type {
  ProjectContextGraphIsland,
  ProjectContextGraphModel,
} from "@/features/project-context/graph";

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

const BASE_COORDINATE_WIDTH = 224;
const BASE_COORDINATE_HEIGHT = 88;
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

function dimensions(kind: ProjectContextLayoutNode["kind"], scale: number) {
  if (kind === "hub") {
    return { width: BASE_HUB_SIZE * scale, height: BASE_HUB_SIZE * scale };
  }
  return {
    width: BASE_COORDINATE_WIDTH * scale,
    height: BASE_COORDINATE_HEIGHT * scale,
  };
}

function nodeKind(id: string): ProjectContextLayoutNode["kind"] {
  return id.startsWith("edge-hub:") ? "hub" : "coordinate";
}

function adjacencyForIsland(
  graph: ProjectContextGraphModel,
  island: ProjectContextGraphIsland,
) {
  const nodeIds = new Set<string>();
  const adjacency = new Map<string, Set<string>>();
  const edgeKeys = new Set(island.edgeKeys);

  function ensure(id: string) {
    nodeIds.add(id);
    if (!adjacency.has(id)) adjacency.set(id, new Set());
  }

  for (const spoke of graph.spokes) {
    if (!edgeKeys.has(spoke.edgeKey)) continue;
    ensure(spoke.sourceId);
    ensure(spoke.targetId);
    adjacency.get(spoke.sourceId)?.add(spoke.targetId);
    adjacency.get(spoke.targetId)?.add(spoke.sourceId);
  }

  return { adjacency, nodeIds };
}

function rootNodeId(nodeIds: Set<string>, adjacency: Map<string, Set<string>>) {
  return [...nodeIds].sort((left, right) => {
    const degreeDifference =
      (adjacency.get(right)?.size ?? 0) - (adjacency.get(left)?.size ?? 0);
    if (degreeDifference !== 0) return degreeDifference;
    const kindDifference =
      Number(nodeKind(right) === "hub") - Number(nodeKind(left) === "hub");
    return kindDifference || compareText(left, right);
  })[0];
}

function layeredNodeIds(
  nodeIds: Set<string>,
  adjacency: Map<string, Set<string>>,
) {
  const root = rootNodeId(nodeIds, adjacency);
  if (!root) return [];
  const distance = new Map([[root, 0]]);
  const queue = [root];

  while (queue.length > 0) {
    const current = queue.shift();
    if (!current) continue;
    const nextDistance = (distance.get(current) ?? 0) + 1;
    for (const neighbor of [...(adjacency.get(current) ?? [])].sort(
      compareText,
    )) {
      if (distance.has(neighbor)) continue;
      distance.set(neighbor, nextDistance);
      queue.push(neighbor);
    }
  }

  const maxDistance = Math.max(...distance.values());
  const layers = Array.from({ length: maxDistance + 1 }, () => [] as string[]);
  for (const id of [...nodeIds].sort(compareText)) {
    layers[distance.get(id) ?? 0].push(id);
  }

  function reorderLayer(layerIndex: number, neighborLayerIndex: number): void {
    const neighborOrder = new Map(
      layers[neighborLayerIndex].map((id, index) => [id, index]),
    );
    layers[layerIndex].sort((left, right) => {
      const barycenter = (id: string) => {
        const indexes = [...(adjacency.get(id) ?? [])]
          .map((neighbor) => neighborOrder.get(neighbor))
          .filter((index): index is number => index !== undefined);
        return indexes.length > 0
          ? indexes.reduce((sum, index) => sum + index, 0) / indexes.length
          : Number.POSITIVE_INFINITY;
      };
      return barycenter(left) - barycenter(right) || compareText(left, right);
    });
  }

  for (let pass = 0; pass < 3; pass += 1) {
    for (let index = 1; index < layers.length; index += 1) {
      reorderLayer(index, index - 1);
    }
    for (let index = layers.length - 2; index >= 0; index -= 1) {
      reorderLayer(index, index + 1);
    }
  }

  return layers;
}

function layoutOneIsland(
  graph: ProjectContextGraphModel,
  island: ProjectContextGraphIsland,
  scale: number,
) {
  const { adjacency, nodeIds } = adjacencyForIsland(graph, island);
  const layers = layeredNodeIds(nodeIds, adjacency);
  const rowGap = BASE_ROW_GAP * scale;
  const layerGap = BASE_LAYER_GAP * scale;
  const paddingX = BASE_ISLAND_PADDING_X * scale;
  const paddingTop = BASE_ISLAND_PADDING_TOP * scale;
  const paddingBottom = BASE_ISLAND_PADDING_BOTTOM * scale;
  const layerWidths = layers.map((layer) =>
    Math.max(...layer.map((id) => dimensions(nodeKind(id), scale).width)),
  );
  const layerHeights = layers.map((layer) =>
    layer.reduce(
      (height, id, index) =>
        height +
        dimensions(nodeKind(id), scale).height +
        (index === 0 ? 0 : rowGap),
      0,
    ),
  );
  const innerWidth =
    layerWidths.reduce((sum, width) => sum + width, 0) +
    Math.max(0, layers.length - 1) * layerGap;
  const innerHeight = Math.max(...layerHeights);
  const width = Math.max(
    BASE_MIN_ISLAND_WIDTH * scale,
    innerWidth + paddingX * 2,
  );
  const height = innerHeight + paddingTop + paddingBottom;
  let layerX = paddingX + (width - paddingX * 2 - innerWidth) / 2;
  const nodes: ProjectContextLayoutNode[] = [];

  layers.forEach((layer, layerIndex) => {
    let nodeY = paddingTop + (innerHeight - layerHeights[layerIndex]) / 2;
    for (const id of layer) {
      const kind = nodeKind(id);
      const size = dimensions(kind, scale);
      nodes.push({
        id,
        kind,
        islandKey: island.stableKey,
        islandIndex: island.index,
        x: layerX + (layerWidths[layerIndex] - size.width) / 2,
        y: nodeY,
        ...size,
      });
      nodeY += size.height + rowGap;
    }
    layerX += layerWidths[layerIndex] + layerGap;
  });

  return { island, nodes, width, height };
}

function layoutFocusedGraph(
  graph: ProjectContextGraphModel,
  scale: number,
): ProjectContextLayout {
  const anchorKeys = new Set(graph.anchorCoordinateKeys);
  const coordinateIds = graph.coordinates.map((coordinate) => coordinate.id);
  const anchorIds = graph.coordinates
    .filter((coordinate) => anchorKeys.has(coordinate.coordinateKey))
    .map((coordinate) => coordinate.id);
  const extraCoordinateIds = graph.coordinates
    .filter((coordinate) => !anchorKeys.has(coordinate.coordinateKey))
    .map((coordinate) => coordinate.id);
  const hubIds = graph.hubs.map((hub) => hub.id);
  const columns = [anchorIds, hubIds, extraCoordinateIds].filter(
    (column) => column.length > 0,
  );
  if (columns.length === 0 && coordinateIds.length === 0) {
    return {
      nodes: [],
      spokes: [],
      islands: [],
      bounds: { x: 0, y: 0, width: 0, height: 0 },
    };
  }

  const rowGap = BASE_ROW_GAP * scale;
  const columnGap = BASE_LAYER_GAP * scale;
  const outer = BASE_OUTER_PADDING * scale;
  const columnWidths = columns.map((column) =>
    Math.max(...column.map((id) => dimensions(nodeKind(id), scale).width)),
  );
  const columnHeights = columns.map((column) =>
    column.reduce(
      (height, id, index) =>
        height +
        dimensions(nodeKind(id), scale).height +
        (index === 0 ? 0 : rowGap),
      0,
    ),
  );
  const innerHeight = Math.max(...columnHeights);
  const nodes: ProjectContextLayoutNode[] = [];
  let columnX = outer;
  columns.forEach((column, columnIndex) => {
    let nodeY = outer + (innerHeight - columnHeights[columnIndex]) / 2;
    for (const id of column) {
      const kind = nodeKind(id);
      const size = dimensions(kind, scale);
      nodes.push({
        id,
        kind,
        islandKey: "query-focus",
        islandIndex: 1,
        x: columnX + (columnWidths[columnIndex] - size.width) / 2,
        y: nodeY,
        ...size,
      });
      nodeY += size.height + rowGap;
    }
    columnX += columnWidths[columnIndex] + columnGap;
  });

  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const spokes = graph.spokes.map((spoke) => {
    const source = nodeById.get(spoke.sourceId);
    const target = nodeById.get(spoke.targetId);
    const port = source && target ? sourcePort(source, target) : "right";
    return {
      id: spoke.id,
      sourceHandle: port,
      targetHandle: oppositePort(port),
    };
  });
  const width =
    outer * 2 +
    columnWidths.reduce((sum, value) => sum + value, 0) +
    Math.max(0, columns.length - 1) * columnGap;
  const height = outer * 2 + innerHeight;
  return {
    nodes,
    spokes,
    islands: [],
    bounds: { x: 0, y: 0, width, height },
  };
}

function oppositePort(port: ProjectContextPort): ProjectContextPort {
  switch (port) {
    case "top":
      return "bottom";
    case "right":
      return "left";
    case "bottom":
      return "top";
    case "left":
      return "right";
  }
}

function sourcePort(
  source: ProjectContextLayoutNode,
  target: ProjectContextLayoutNode,
): ProjectContextPort {
  const sourceX = source.x + source.width / 2;
  const sourceY = source.y + source.height / 2;
  const targetX = target.x + target.width / 2;
  const targetY = target.y + target.height / 2;
  const deltaX = targetX - sourceX;
  const deltaY = targetY - sourceY;
  if (Math.abs(deltaX) >= Math.abs(deltaY)) {
    return deltaX >= 0 ? "right" : "left";
  }
  return deltaY >= 0 ? "bottom" : "top";
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

/**
 * Deterministically lay out each incidence component, then pack Islands with
 * explicit whitespace. No force simulation or persistent presentation state
 * is involved.
 */
export function layoutProjectContextGraph(
  graph: ProjectContextGraphModel,
  requestedScale = 1,
): ProjectContextLayout {
  const scale = Number.isFinite(requestedScale)
    ? Math.min(Math.max(requestedScale, 0.75), 1.5)
    : 1;
  if (graph.isAllContext === false) {
    return layoutFocusedGraph(graph, scale);
  }
  if (graph.islands.length === 0) {
    return {
      nodes: [],
      spokes: [],
      islands: [],
      bounds: { x: 0, y: 0, width: 0, height: 0 },
    };
  }

  const layouts = graph.islands.map((island) =>
    layoutOneIsland(graph, island, scale),
  );
  const columnCount = Math.ceil(Math.sqrt(layouts.length));
  const rowCount = Math.ceil(layouts.length / columnCount);
  const columnWidths = Array.from({ length: columnCount }, () => 0);
  const rowHeights = Array.from({ length: rowCount }, () => 0);
  layouts.forEach((layout, index) => {
    const column = index % columnCount;
    const row = Math.floor(index / columnCount);
    columnWidths[column] = Math.max(columnWidths[column], layout.width);
    rowHeights[row] = Math.max(rowHeights[row], layout.height);
  });

  const gap = BASE_ISLAND_GAP * scale;
  const outer = BASE_OUTER_PADDING * scale;
  const columnX = columnWidths.map(
    (_, index) =>
      outer +
      columnWidths.slice(0, index).reduce((sum, width) => sum + width, 0) +
      index * gap,
  );
  const rowY = rowHeights.map(
    (_, index) =>
      outer +
      rowHeights.slice(0, index).reduce((sum, height) => sum + height, 0) +
      index * gap,
  );
  const nodes: ProjectContextLayoutNode[] = [];
  const islands: ProjectContextIslandLayout[] = [];

  layouts.forEach((layout, index) => {
    const column = index % columnCount;
    const row = Math.floor(index / columnCount);
    const x = columnX[column] + (columnWidths[column] - layout.width) / 2;
    const y = rowY[row] + (rowHeights[row] - layout.height) / 2;
    islands.push({
      ...layout.island,
      bounds: { x, y, width: layout.width, height: layout.height },
    });
    nodes.push(
      ...layout.nodes.map((node) => ({
        ...node,
        x: node.x + x,
        y: node.y + y,
      })),
    );
  });

  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const spokes = graph.spokes.map((spoke) => {
    const source = nodeById.get(spoke.sourceId);
    const target = nodeById.get(spoke.targetId);
    const port = source && target ? sourcePort(source, target) : "right";
    return {
      id: spoke.id,
      sourceHandle: port,
      targetHandle: oppositePort(port),
    };
  });
  const width =
    outer * 2 +
    columnWidths.reduce((sum, value) => sum + value, 0) +
    Math.max(0, columnCount - 1) * gap;
  const height =
    outer * 2 +
    rowHeights.reduce((sum, value) => sum + value, 0) +
    Math.max(0, rowCount - 1) * gap;

  return {
    nodes,
    spokes,
    islands,
    bounds: { x: 0, y: 0, width, height },
  };
}
