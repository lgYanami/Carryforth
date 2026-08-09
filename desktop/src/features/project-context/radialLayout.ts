export type RadialLayoutNodeInput = {
  id: string;
  kind: "coordinate" | "hub";
  width: number;
  height: number;
};

export type RadialLayoutLinkInput = {
  sourceId: string;
  targetId: string;
};

export type RadialComponentInput = {
  stableKey: string;
  nodes: readonly RadialLayoutNodeInput[];
  links: readonly RadialLayoutLinkInput[];
  centerIds: readonly string[];
  virtualCenter: boolean;
};

export type RadialLayoutPosition = {
  id: string;
  centerX: number;
  centerY: number;
  depth: number;
  band: number;
};

export type RadialLayoutDiagnostics = {
  collisionPairs: number;
  ticks: number;
  usedSafeSeed: boolean;
};

export type RadialComponentLayout = {
  positions: RadialLayoutPosition[];
  diagnostics: RadialLayoutDiagnostics;
};

const TAU = Math.PI * 2;
const RING_FILL_RATIO = 0.72;
const TANGENTIAL_GAP = 38;
const RADIAL_GAP = 72;
const LINK_GAP = 64;
const COLLISION_GAP = 24;
const POSITION_QUANTUM = 0.25;

function compareText(left: string, right: string) {
  return left.localeCompare(right, "en");
}

function stableHash(value: string) {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

function compareStableAngle(left: string, right: string) {
  return stableHash(left) - stableHash(right) || compareText(left, right);
}

function quantize(value: number) {
  if (!Number.isFinite(value)) return value;
  const result = Math.round(value / POSITION_QUANTUM) * POSITION_QUANTUM;
  return Object.is(result, -0) ? 0 : result;
}

function outerRadius(node: RadialLayoutNodeInput) {
  return Math.hypot(node.width, node.height) / 2;
}

function sortedUnique(values: readonly string[]) {
  return [...new Set(values)].sort(compareText);
}

function buildAdjacency(
  nodes: readonly RadialLayoutNodeInput[],
  links: readonly RadialLayoutLinkInput[],
) {
  const nodeIds = new Set(nodes.map((node) => node.id));
  const adjacency = new Map<string, Set<string>>();
  for (const id of nodeIds) adjacency.set(id, new Set());
  for (const link of links) {
    if (!nodeIds.has(link.sourceId) || !nodeIds.has(link.targetId)) continue;
    adjacency.get(link.sourceId)?.add(link.targetId);
    adjacency.get(link.targetId)?.add(link.sourceId);
  }
  return adjacency;
}

function bfsForest(
  nodes: readonly RadialLayoutNodeInput[],
  adjacency: ReadonlyMap<string, Set<string>>,
  requestedCenters: readonly string[],
) {
  const allIds = nodes.map((node) => node.id).sort(compareText);
  const nodeIds = new Set(allIds);
  const centers = sortedUnique(requestedCenters).filter((id) =>
    nodeIds.has(id),
  );
  if (centers.length === 0 && allIds[0]) centers.push(allIds[0]);
  const depth = new Map<string, number>();
  const parent = new Map<string, string | undefined>();
  const queue: string[] = [];

  for (const center of centers) {
    depth.set(center, 0);
    parent.set(center, undefined);
    queue.push(center);
  }

  let queueIndex = 0;
  while (queueIndex < queue.length) {
    const current = queue[queueIndex];
    queueIndex += 1;
    const nextDepth = (depth.get(current) ?? 0) + 1;
    for (const neighbor of [...(adjacency.get(current) ?? [])].sort(
      compareText,
    )) {
      const previousDepth = depth.get(neighbor);
      if (previousDepth === undefined) {
        depth.set(neighbor, nextDepth);
        parent.set(neighbor, current);
        queue.push(neighbor);
      } else if (
        previousDepth === nextDepth &&
        compareText(current, parent.get(neighbor) ?? current) < 0
      ) {
        parent.set(neighbor, current);
      }
    }
  }

  for (const id of allIds) {
    if (depth.has(id)) continue;
    depth.set(id, 0);
    parent.set(id, undefined);
    centers.push(id);
  }

  return { centers: sortedUnique(centers), depth, parent };
}

type MutablePosition = RadialLayoutPosition & {
  initialX: number;
  initialY: number;
};

function seedBands(
  input: RadialComponentInput,
  nodeById: ReadonlyMap<string, RadialLayoutNodeInput>,
  depthById: ReadonlyMap<string, number>,
  parentById: ReadonlyMap<string, string | undefined>,
  centers: readonly string[],
) {
  const positions = new Map<string, MutablePosition>();
  const realPinnedCenter = !input.virtualCenter && centers.length === 1;
  if (realPinnedCenter) {
    const centerId = centers[0];
    positions.set(centerId, {
      id: centerId,
      centerX: 0,
      centerY: 0,
      initialX: 0,
      initialY: 0,
      depth: 0,
      band: 0,
    });
  }

  const nodesByVisualDepth = new Map<number, RadialLayoutNodeInput[]>();
  for (const node of input.nodes) {
    if (realPinnedCenter && node.id === centers[0]) continue;
    const visualDepth =
      (depthById.get(node.id) ?? 0) + (input.virtualCenter ? 1 : 0);
    const layer = nodesByVisualDepth.get(visualDepth) ?? [];
    layer.push(node);
    nodesByVisualDepth.set(visualDepth, layer);
  }

  const rotation = (stableHash(input.stableKey) / 0x1_0000_0000) * TAU;
  let previousRadius = realPinnedCenter
    ? outerRadius(nodeById.get(centers[0]) ?? input.nodes[0])
    : 0;
  let previousOuter = previousRadius;
  let globalBand = realPinnedCenter ? 1 : 0;

  const parentAngle = new Map<string, number>();
  if (realPinnedCenter) parentAngle.set(centers[0], rotation);

  for (const visualDepth of [...nodesByVisualDepth.keys()].sort(
    (a, b) => a - b,
  )) {
    const layer = nodesByVisualDepth.get(visualDepth) ?? [];
    layer.sort((left, right) => {
      const leftParent = parentById.get(left.id);
      const rightParent = parentById.get(right.id);
      const leftAngle = leftParent ? parentAngle.get(leftParent) : undefined;
      const rightAngle = rightParent ? parentAngle.get(rightParent) : undefined;
      if (
        leftAngle !== undefined &&
        rightAngle !== undefined &&
        leftAngle !== rightAngle
      ) {
        return leftAngle - rightAngle;
      }
      return compareStableAngle(left.id, right.id);
    });
    const maxOuter = Math.max(...layer.map(outerRadius));
    const radialPitch = maxOuter * 2 + RADIAL_GAP;
    const baseRadius = Math.max(
      maxOuter + RADIAL_GAP,
      previousRadius + previousOuter + maxOuter + RADIAL_GAP,
    );
    let remainingIndex = 0;
    let depthBand = 0;

    while (remainingIndex < layer.length) {
      const radius = baseRadius + depthBand * radialPitch;
      const capacity = Math.max(1, TAU * radius * RING_FILL_RATIO);
      const bandNodes: RadialLayoutNodeInput[] = [];
      let used = 0;
      while (remainingIndex < layer.length) {
        const candidate = layer[remainingIndex];
        const demand = outerRadius(candidate) * 2 + TANGENTIAL_GAP;
        if (bandNodes.length > 0 && used + demand > capacity) break;
        bandNodes.push(candidate);
        used += demand;
        remainingIndex += 1;
      }

      const totalDemand = bandNodes.reduce(
        (sum, node) => sum + outerRadius(node) * 2 + TANGENTIAL_GAP,
        0,
      );
      let consumed = 0;
      for (const node of bandNodes) {
        const demand = outerRadius(node) * 2 + TANGENTIAL_GAP;
        const angle = rotation + ((consumed + demand / 2) / totalDemand) * TAU;
        consumed += demand;
        const centerX = Math.cos(angle) * radius;
        const centerY = Math.sin(angle) * radius;
        positions.set(node.id, {
          id: node.id,
          centerX,
          centerY,
          initialX: centerX,
          initialY: centerY,
          depth: depthById.get(node.id) ?? 0,
          band: globalBand,
        });
        parentAngle.set(node.id, angle);
      }
      previousRadius = radius;
      previousOuter = maxOuter;
      depthBand += 1;
      globalBand += 1;
    }
  }

  return positions;
}

type Rect = {
  id: string;
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
};

function rectFor(
  node: RadialLayoutNodeInput,
  position: Pick<MutablePosition, "centerX" | "centerY">,
  gap = 0,
): Rect {
  return {
    id: node.id,
    minX: position.centerX - node.width / 2 - gap / 2,
    minY: position.centerY - node.height / 2 - gap / 2,
    maxX: position.centerX + node.width / 2 + gap / 2,
    maxY: position.centerY + node.height / 2 + gap / 2,
  };
}

function rectsOverlap(left: Rect, right: Rect) {
  return (
    left.minX < right.maxX &&
    left.maxX > right.minX &&
    left.minY < right.maxY &&
    left.maxY > right.minY
  );
}

function candidatePairs(
  nodes: readonly RadialLayoutNodeInput[],
  positions: ReadonlyMap<string, MutablePosition>,
  gap: number,
  pairBudget: number,
) {
  const maxDimension = Math.max(
    1,
    ...nodes.map((node) => Math.max(node.width, node.height) + gap),
  );
  const buckets = new Map<string, string[]>();
  const rectById = new Map<string, Rect>();
  for (const node of nodes) {
    const position = positions.get(node.id);
    if (!position) continue;
    const rect = rectFor(node, position, gap);
    rectById.set(node.id, rect);
    const minCellX = Math.floor(rect.minX / maxDimension);
    const maxCellX = Math.floor(rect.maxX / maxDimension);
    const minCellY = Math.floor(rect.minY / maxDimension);
    const maxCellY = Math.floor(rect.maxY / maxDimension);
    for (let cellX = minCellX; cellX <= maxCellX; cellX += 1) {
      for (let cellY = minCellY; cellY <= maxCellY; cellY += 1) {
        const key = `${cellX}:${cellY}`;
        const bucket = buckets.get(key) ?? [];
        bucket.push(node.id);
        buckets.set(key, bucket);
      }
    }
  }

  const pairKeys = new Set<string>();
  const pairs: Array<[string, string]> = [];
  for (const key of [...buckets.keys()].sort(compareText)) {
    const ids = sortedUnique(buckets.get(key) ?? []);
    for (let left = 0; left < ids.length; left += 1) {
      for (let right = left + 1; right < ids.length; right += 1) {
        const pairKey = `${ids[left]}\u0000${ids[right]}`;
        if (pairKeys.has(pairKey)) continue;
        pairKeys.add(pairKey);
        if (pairs.length >= pairBudget) {
          return { pairs, rectById, exhausted: true };
        }
        pairs.push([ids[left], ids[right]]);
      }
    }
  }
  return { pairs, rectById, exhausted: false };
}

function hasOverlap(
  nodes: readonly RadialLayoutNodeInput[],
  positions: ReadonlyMap<string, MutablePosition>,
) {
  const candidates = candidatePairs(
    nodes,
    positions,
    0,
    Math.max(4_096, nodes.length * 64),
  );
  if (candidates.exhausted) return true;
  return candidates.pairs.some(([leftId, rightId]) => {
    const left = candidates.rectById.get(leftId);
    const right = candidates.rectById.get(rightId);
    return (
      left !== undefined && right !== undefined && rectsOverlap(left, right)
    );
  });
}

function expandSeed(
  positions: Map<string, MutablePosition>,
  pinnedId: string | undefined,
) {
  for (const position of positions.values()) {
    if (position.id === pinnedId) continue;
    position.centerX *= 1.12;
    position.centerY *= 1.12;
    position.initialX = position.centerX;
    position.initialY = position.centerY;
  }
}

function clonePositions(positions: ReadonlyMap<string, MutablePosition>) {
  return new Map([...positions].map(([id, position]) => [id, { ...position }]));
}

function isAnalyticFastPath(
  nodes: readonly RadialLayoutNodeInput[],
  links: readonly RadialLayoutLinkInput[],
  pinnedCenterId: string | undefined,
) {
  if (nodes.length <= 2) return true;
  if (!pinnedCenterId || links.length !== nodes.length - 1) return false;
  return links.every(
    (link) =>
      link.sourceId === pinnedCenterId || link.targetId === pinnedCenterId,
  );
}

function relax(
  input: RadialComponentInput,
  nodes: readonly RadialLayoutNodeInput[],
  nodeById: ReadonlyMap<string, RadialLayoutNodeInput>,
  seed: ReadonlyMap<string, MutablePosition>,
  pinnedId: string | undefined,
) {
  const positions = clonePositions(seed);
  const sortedLinks = [...input.links].sort(
    (left, right) =>
      compareText(left.sourceId, right.sourceId) ||
      compareText(left.targetId, right.targetId),
  );
  const complexity = nodes.length + sortedLinks.length;
  const ticks = complexity <= 240 ? 64 : complexity <= 900 ? 40 : 20;
  const pairBudget = Math.max(4_096, nodes.length * 64);
  let collisionPairs = 0;

  for (let tick = 0; tick < ticks; tick += 1) {
    const force = new Map(nodes.map((node) => [node.id, { x: 0, y: 0 }]));
    for (const link of sortedLinks) {
      const source = positions.get(link.sourceId);
      const target = positions.get(link.targetId);
      const sourceNode = nodeById.get(link.sourceId);
      const targetNode = nodeById.get(link.targetId);
      if (!source || !target || !sourceNode || !targetNode) continue;
      let deltaX = target.centerX - source.centerX;
      let deltaY = target.centerY - source.centerY;
      let distance = Math.hypot(deltaX, deltaY);
      if (distance < 0.001) {
        const angle =
          (stableHash(`${link.sourceId}:${link.targetId}`) / 0x1_0000_0000) *
          TAU;
        deltaX = Math.cos(angle);
        deltaY = Math.sin(angle);
        distance = 1;
      }
      const ideal =
        outerRadius(sourceNode) + outerRadius(targetNode) + LINK_GAP;
      const magnitude = (distance - ideal) * 0.035;
      const unitX = deltaX / distance;
      const unitY = deltaY / distance;
      const sourceForce = force.get(link.sourceId);
      const targetForce = force.get(link.targetId);
      if (sourceForce) {
        sourceForce.x += unitX * magnitude;
        sourceForce.y += unitY * magnitude;
      }
      if (targetForce) {
        targetForce.x -= unitX * magnitude;
        targetForce.y -= unitY * magnitude;
      }
    }

    for (const position of positions.values()) {
      const nodeForce = force.get(position.id);
      if (!nodeForce || position.id === pinnedId) continue;
      nodeForce.x += (position.initialX - position.centerX) * 0.045;
      nodeForce.y += (position.initialY - position.centerY) * 0.045;
    }

    const candidates = candidatePairs(
      nodes,
      positions,
      COLLISION_GAP,
      pairBudget,
    );
    if (candidates.exhausted) {
      return {
        positions: clonePositions(seed),
        ticks,
        collisionPairs,
        usedSafeSeed: true,
      };
    }
    collisionPairs += candidates.pairs.length;
    for (const [leftId, rightId] of candidates.pairs) {
      const leftRect = candidates.rectById.get(leftId);
      const rightRect = candidates.rectById.get(rightId);
      if (!leftRect || !rightRect || !rectsOverlap(leftRect, rightRect))
        continue;
      const left = positions.get(leftId);
      const right = positions.get(rightId);
      if (!left || !right) continue;
      const overlapX =
        Math.min(leftRect.maxX, rightRect.maxX) -
        Math.max(leftRect.minX, rightRect.minX);
      const overlapY =
        Math.min(leftRect.maxY, rightRect.maxY) -
        Math.max(leftRect.minY, rightRect.minY);
      const leftForce = force.get(leftId);
      const rightForce = force.get(rightId);
      if (!leftForce || !rightForce) continue;
      if (overlapX <= overlapY) {
        const direction = left.centerX <= right.centerX ? -1 : 1;
        const push = (overlapX + 1) * 0.52;
        if (leftId !== pinnedId) leftForce.x += direction * push;
        if (rightId !== pinnedId) rightForce.x -= direction * push;
      } else {
        const direction = left.centerY <= right.centerY ? -1 : 1;
        const push = (overlapY + 1) * 0.52;
        if (leftId !== pinnedId) leftForce.y += direction * push;
        if (rightId !== pinnedId) rightForce.y -= direction * push;
      }
    }

    const displacementCap = 18 * (1 - (tick / Math.max(1, ticks - 1)) * 0.65);
    for (const node of nodes) {
      if (node.id === pinnedId) continue;
      const position = positions.get(node.id);
      const nodeForce = force.get(node.id);
      if (!position || !nodeForce) continue;
      const magnitude = Math.hypot(nodeForce.x, nodeForce.y);
      const scale =
        magnitude > displacementCap ? displacementCap / magnitude : 1;
      position.centerX += nodeForce.x * scale;
      position.centerY += nodeForce.y * scale;
    }
    if (pinnedId) {
      const pinned = positions.get(pinnedId);
      if (pinned) {
        pinned.centerX = 0;
        pinned.centerY = 0;
      }
    }
  }

  if (hasOverlap(nodes, positions)) {
    return {
      positions: clonePositions(seed),
      ticks,
      collisionPairs,
      usedSafeSeed: true,
    };
  }
  return { positions, ticks, collisionPairs, usedSafeSeed: false };
}

function finalize(
  nodes: readonly RadialLayoutNodeInput[],
  candidate: ReadonlyMap<string, MutablePosition>,
  safeSeed: ReadonlyMap<string, MutablePosition>,
) {
  const quantized = clonePositions(candidate);
  for (const position of quantized.values()) {
    position.centerX = quantize(position.centerX);
    position.centerY = quantize(position.centerY);
  }
  if (
    [...quantized.values()].some(
      (position) =>
        !Number.isFinite(position.centerX) ||
        !Number.isFinite(position.centerY),
    ) ||
    hasOverlap(nodes, quantized)
  ) {
    const fallback = clonePositions(safeSeed);
    for (const position of fallback.values()) {
      position.centerX = quantize(position.centerX);
      position.centerY = quantize(position.centerY);
    }
    return { positions: fallback, usedSafeSeed: true };
  }
  return { positions: quantized, usedSafeSeed: false };
}

/**
 * Computes a deterministic, finite radial component layout before React Flow
 * renders. The returned positions are frozen presentation data, not a live
 * simulation and not persisted Project Context state.
 */
export function layoutRadialComponent(
  input: RadialComponentInput,
): RadialComponentLayout | null {
  const nodes = [...input.nodes].sort((left, right) =>
    compareText(left.id, right.id),
  );
  if (nodes.length === 0) {
    return {
      positions: [],
      diagnostics: { collisionPairs: 0, ticks: 0, usedSafeSeed: false },
    };
  }
  const nodeById = new Map(nodes.map((node) => [node.id, node]));
  const adjacency = buildAdjacency(nodes, input.links);
  const forest = bfsForest(nodes, adjacency, input.centerIds);
  const pinnedId =
    !input.virtualCenter && forest.centers.length === 1
      ? forest.centers[0]
      : undefined;
  const seed = seedBands(
    input,
    nodeById,
    forest.depth,
    forest.parent,
    forest.centers,
  );
  for (let pass = 0; pass < 3 && hasOverlap(nodes, seed); pass += 1) {
    expandSeed(seed, pinnedId);
  }
  if (hasOverlap(nodes, seed)) return null;

  const result = isAnalyticFastPath(nodes, input.links, pinnedId)
    ? {
        positions: clonePositions(seed),
        ticks: 0,
        collisionPairs: 0,
        usedSafeSeed: false,
      }
    : relax(input, nodes, nodeById, seed, pinnedId);
  const finalized = finalize(nodes, result.positions, seed);
  const usedSafeSeed = result.usedSafeSeed || finalized.usedSafeSeed;
  return {
    positions: [...finalized.positions.values()]
      .map(
        ({ initialX: _initialX, initialY: _initialY, ...position }) => position,
      )
      .sort(
        (left, right) =>
          left.depth - right.depth ||
          left.band - right.band ||
          compareText(left.id, right.id),
      ),
    diagnostics: {
      collisionPairs: result.collisionPairs,
      ticks: result.ticks,
      usedSafeSeed,
    },
  };
}
