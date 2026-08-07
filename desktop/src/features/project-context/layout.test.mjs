import assert from "node:assert/strict";
import test from "node:test";

import { buildProjectContextGraph } from "./graph.ts";
import {
  layoutProjectContextGraph,
  projectContextBoundsOverlap,
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

test("every Spoke receives opposite boundary ports", () => {
  const layout = layoutProjectContextGraph(graphFixture());
  const opposite = {
    top: "bottom",
    right: "left",
    bottom: "top",
    left: "right",
  };
  assert.equal(layout.spokes.length, 7);
  for (const spoke of layout.spokes) {
    assert.equal(spoke.targetHandle, opposite[spoke.sourceHandle]);
  }
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
