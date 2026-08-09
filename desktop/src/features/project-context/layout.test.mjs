import assert from "node:assert/strict";
import test from "node:test";

import { buildProjectContextGraph } from "./graph.ts";
import {
  buildProjectContextLayoutTopology,
  layoutProjectContextGeometry,
  layoutProjectContextGraph,
  materializeProjectContextLayout,
  projectContextBoundsOverlap,
  projectContextPortToward,
} from "./layout.ts";

function detail(coordinateKey) {
  const [objectType, objectId] = coordinateKey.split(":");
  return {
    coordinateKey,
    coordinate: {
      type: "project_view_object",
      objectType,
      objectId,
    },
    state: "active",
    title: `${objectType} ${objectId}`,
  };
}

function graphFixture() {
  const edges = [
    {
      edgeKey: "edge-ab",
      coordinateKeys: ["requirement:a", "resource:b"],
      contextDocumentIds: ["context-ab"],
    },
    {
      edgeKey: "edge-abc",
      coordinateKeys: ["requirement:a", "resource:b", "goal:c"],
      contextDocumentIds: ["context-abc"],
    },
    {
      edgeKey: "edge-de",
      coordinateKeys: ["role:d", "work:e"],
      contextDocumentIds: ["context-de"],
    },
  ];
  const coordinateDetails = [
    "requirement:a",
    "resource:b",
    "goal:c",
    "role:d",
    "work:e",
  ].map(detail);
  return buildProjectContextGraph({
    communityKey: "community-0",
    projectId: "project",
    relayPubkey: "a".repeat(64),
    context: {
      contextRevision: 1,
      projectionGeneration: 1,
      activeEdgeCount: edges.length,
      boundDocumentCount: 3,
      updatedAt: "2026-08-06T00:00:00Z",
      metaEventId: "b".repeat(64),
      capabilityEnabled: true,
    },
    query: { type: "contains_all", coordinates: [] },
    projectViewObservation: { state: "observed" },
    documentObservation: { state: "observed" },
    edges,
    coordinateDetails,
    documentDetails: [],
  });
}

function nodeBounds(node) {
  return {
    x: node.x,
    y: node.y,
    width: node.width,
    height: node.height,
  };
}

test("deterministic layout is byte-for-byte stable", () => {
  const graph = graphFixture();
  assert.deepEqual(
    layoutProjectContextGraph(graph),
    layoutProjectContextGraph(graph),
  );
});

test("packed Island bounds never overlap", () => {
  const layout = layoutProjectContextGraph(graphFixture());
  assert.equal(layout.islands.length, 2);
  for (let left = 0; left < layout.islands.length; left += 1) {
    for (let right = left + 1; right < layout.islands.length; right += 1) {
      assert.equal(
        projectContextBoundsOverlap(
          layout.islands[left].bounds,
          layout.islands[right].bounds,
        ),
        false,
      );
    }
  }
});

test("Coordinate and Hub rectangles have deterministic reading space", () => {
  const layout = layoutProjectContextGraph(graphFixture());
  for (const island of layout.islands) {
    const islandNodes = layout.nodes.filter(
      (node) => node.islandKey === island.stableKey,
    );
    for (let left = 0; left < islandNodes.length; left += 1) {
      for (let right = left + 1; right < islandNodes.length; right += 1) {
        assert.equal(
          projectContextBoundsOverlap(
            nodeBounds(islandNodes[left]),
            nodeBounds(islandNodes[right]),
          ),
          false,
          `${islandNodes[left].id} overlaps ${islandNodes[right].id}`,
        );
      }
    }
  }
});

test("every Spoke chooses each endpoint Handle from its own rectangle", () => {
  const graph = graphFixture();
  const layout = layoutProjectContextGraph(graph);
  const nodeById = new Map(layout.nodes.map((node) => [node.id, node]));
  const graphSpokeById = new Map(
    graph.spokes.map((spoke) => [spoke.id, spoke]),
  );
  assert.equal(layout.spokes.length, 7);
  for (const spoke of layout.spokes) {
    const graphSpoke = graphSpokeById.get(spoke.id);
    assert.ok(graphSpoke);
    assert.equal(
      spoke.sourceHandle,
      projectContextPortToward(
        nodeById.get(graphSpoke.sourceId),
        nodeById.get(graphSpoke.targetId),
      ),
    );
    assert.equal(
      spoke.targetHandle,
      projectContextPortToward(
        nodeById.get(graphSpoke.targetId),
        nodeById.get(graphSpoke.sourceId),
      ),
    );
  }
  const square = { x: 0, y: 0, width: 76, height: 76 };
  const wide = { x: 100, y: 90, width: 224, height: 120 };
  assert.equal(projectContextPortToward(square, wide), "right");
  assert.equal(projectContextPortToward(wide, square), "top");
});

test("text scale expands nodes, spacing, and Island bounds together", () => {
  const regular = layoutProjectContextGraph(graphFixture(), 1);
  const enlarged = layoutProjectContextGraph(graphFixture(), 1.5);
  const regularCoordinate = regular.nodes.find(
    (node) => node.kind === "coordinate",
  );
  const enlargedCoordinate = enlarged.nodes.find(
    (node) => node.id === regularCoordinate?.id,
  );
  assert.ok(regularCoordinate);
  assert.ok(enlargedCoordinate);
  assert.equal(enlargedCoordinate.width, regularCoordinate.width * 1.5);
  assert.equal(enlargedCoordinate.height, regularCoordinate.height * 1.5);
  assert.ok(enlarged.bounds.width > regular.bounds.width);
  assert.ok(enlarged.bounds.height > regular.bounds.height);
});

test("one connected Island fans into at least three quadrants", () => {
  const layout = layoutProjectContextGraph(graphFixture());
  const island = layout.islands.find(
    (candidate) => candidate.edgeKeys.length === 2,
  );
  assert.ok(island);
  const nodes = layout.nodes.filter(
    (node) => node.islandKey === island.stableKey,
  );
  const center = nodes.find((node) => node.id === "edge-hub:edge-abc");
  assert.ok(center);
  const centerX = center.x + center.width / 2;
  const centerY = center.y + center.height / 2;
  const quadrants = new Set(
    nodes
      .filter((node) => node.id !== center.id)
      .map((node) => {
        const x = node.x + node.width / 2 >= centerX ? "right" : "left";
        const y = node.y + node.height / 2 >= centerY ? "bottom" : "top";
        return `${x}:${y}`;
      }),
  );
  assert.ok(quadrants.size >= 3);
});

test("metadata changes reuse geometry while Island facts stay current", () => {
  const graph = graphFixture();
  const topology = buildProjectContextLayoutTopology(graph);
  const geometry = layoutProjectContextGeometry(topology);
  const changed = structuredClone(graph);
  changed.coordinates[0].displayTitle = "Changed metadata only";
  changed.coordinates[0].summary = "A newer summary";
  changed.hubs[0].contextDocumentIds.push("new-context-document");
  changed.islands[0].contextDocumentIds.push("new-context-document");
  const changedTopology = buildProjectContextLayoutTopology(changed);

  assert.equal(changedTopology.descriptor, topology.descriptor);
  assert.deepEqual(layoutProjectContextGeometry(changedTopology), geometry);
  const materialized = materializeProjectContextLayout(geometry, changed);
  assert.equal(
    materialized.islands.some((island) =>
      island.contextDocumentIds.includes("new-context-document"),
    ),
    true,
  );
});

test("empty graph has a closed zero layout", () => {
  assert.deepEqual(
    layoutProjectContextGraph({
      coordinates: [],
      hubs: [],
      spokes: [],
      islands: [],
    }),
    {
      nodes: [],
      spokes: [],
      islands: [],
      bounds: { x: 0, y: 0, width: 0, height: 0 },
    },
  );
});

test("focused no-match lays out standalone Anchors without Island bounds", () => {
  const objectId = "60000000-0000-4000-8000-000000000001";
  const graph = buildProjectContextGraph({
    communityKey: "community-0",
    projectId: "project",
    relayPubkey: "a".repeat(64),
    context: {
      contextRevision: 1,
      projectionGeneration: 1,
      activeEdgeCount: 3,
      boundDocumentCount: 3,
      updatedAt: "2026-08-06T00:00:00Z",
      metaEventId: "b".repeat(64),
      capabilityEnabled: true,
    },
    query: {
      type: "incident",
      coordinate: {
        type: "project_view_object",
        objectType: "requirement",
        objectId,
      },
    },
    projectViewObservation: { state: "unavailable" },
    documentObservation: { state: "not_requested" },
    edges: [],
    coordinateDetails: [
      {
        coordinateKey: `requirement:${objectId}`,
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId,
        },
        state: "unavailable",
      },
    ],
    documentDetails: [],
  });
  const layout = layoutProjectContextGraph(graph);

  assert.equal(layout.nodes.length, 1);
  assert.equal(layout.nodes[0].id, `coordinate:requirement:${objectId}`);
  assert.deepEqual(layout.islands, []);
  assert.ok(layout.bounds.width > 0);
  assert.ok(layout.bounds.height > 0);
});

test("focused exact centers its one matching Hub", () => {
  const source = graphFixture();
  const hub = source.hubs.find((candidate) => candidate.edgeKey === "edge-abc");
  assert.ok(hub);
  const coordinateKeySet = new Set(hub.coordinateKeys);
  const graph = {
    ...source,
    anchorCoordinateKeys: [...hub.coordinateKeys],
    coordinates: source.coordinates.filter((coordinate) =>
      coordinateKeySet.has(coordinate.coordinateKey),
    ),
    hubs: [hub],
    isAllContext: false,
    queryMode: "exact",
    spokes: source.spokes.filter((spoke) => spoke.edgeKey === hub.edgeKey),
  };
  const layout = layoutProjectContextGraph(graph);
  const center = layout.nodes.find((node) => node.id === hub.id);
  assert.ok(center);
  const centerX = center.x + center.width / 2;
  const centerY = center.y + center.height / 2;

  for (const node of layout.nodes.filter((node) => node.id !== hub.id)) {
    assert.ok(
      Math.hypot(
        node.x + node.width / 2 - centerX,
        node.y + node.height / 2 - centerY,
      ) > 100,
    );
  }
  assert.deepEqual(layout.islands, []);
});

test("focused contains-all keeps multiple Anchors around a virtual center", () => {
  const source = graphFixture();
  const hub = source.hubs.find((candidate) => candidate.edgeKey === "edge-abc");
  assert.ok(hub);
  const graph = {
    ...source,
    anchorCoordinateKeys: ["requirement:a", "resource:b"],
    coordinates: source.coordinates.filter((coordinate) =>
      hub.coordinateKeys.includes(coordinate.coordinateKey),
    ),
    hubs: [hub],
    isAllContext: false,
    queryMode: "contains_all",
    spokes: source.spokes.filter((spoke) => spoke.edgeKey === hub.edgeKey),
  };
  const layout = layoutProjectContextGraph(graph);
  const anchors = graph.anchorCoordinateKeys.map((key) =>
    layout.nodes.find((node) => node.id === `coordinate:${key}`),
  );
  assert.equal(anchors.every(Boolean), true);
  const virtualX =
    anchors.reduce((sum, node) => sum + node.x + node.width / 2, 0) /
    anchors.length;
  const virtualY =
    anchors.reduce((sum, node) => sum + node.y + node.height / 2, 0) /
    anchors.length;
  const hubNode = layout.nodes.find((node) => node.id === hub.id);
  assert.ok(hubNode);
  assert.ok(
    Math.hypot(
      hubNode.x + hubNode.width / 2 - virtualX,
      hubNode.y + hubNode.height / 2 - virtualY,
    ) > 100,
  );
});
