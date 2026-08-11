import assert from "node:assert/strict";
import test from "node:test";

import { buildProjectContextGraph } from "./graph.ts";
import { layoutProjectContextGraph } from "./layout.ts";
import {
  buildProjectContextFlowElements,
  projectContextIslandHue,
} from "./presentation.ts";

function fixture() {
  const coordinateKeys = ["requirement:a", "resource:b", "goal:c"];
  const graph = buildProjectContextGraph({
    communityKey: "community-0",
    projectId: "project",
    relayPubkey: "a".repeat(64),
    context: {
      contextRevision: 1,
      projectionGeneration: 1,
      activeEdgeCount: 2,
      boundDocumentCount: 2,
      updatedAt: "2026-08-06T00:00:00Z",
      metaEventId: "b".repeat(64),
      capabilityEnabled: true,
    },
    query: { type: "contains_all", coordinates: [] },
    projectViewObservation: { state: "observed" },
    documentObservation: { state: "observed" },
    edges: [
      {
        edgeKey: "edge-ab",
        coordinateKeys: coordinateKeys.slice(0, 2),
        contextDocumentIds: ["context-ab"],
      },
      {
        edgeKey: "edge-abc",
        coordinateKeys,
        contextDocumentIds: ["context-abc"],
      },
    ],
    coordinateDetails: coordinateKeys.map((coordinateKey) => {
      const [objectType, objectId] = coordinateKey.split(":");
      return {
        coordinateKey,
        coordinate: {
          type: "project_view_object",
          objectType,
          objectId,
        },
        state: "active",
        title: coordinateKey,
      };
    }),
    documentDetails: [],
  });
  return { graph, layout: layoutProjectContextGraph(graph) };
}

function emphasisByKey(elements, kind) {
  return Object.fromEntries(
    [...elements.nodes, ...elements.edges]
      .filter((element) => element.data?.kind === kind)
      .map((element) => {
        const data = element.data;
        if (kind === "coordinate") {
          return [data.coordinate.coordinateKey, data.emphasis];
        }
        if (kind === "hub") return [data.hub.edgeKey, data.emphasis];
        return [element.id, data.emphasis];
      }),
  );
}

function semanticOverlay(overrides = {}) {
  return {
    communityKey: "community-0",
    requestId: "request-0",
    projectId: "project",
    relayPubkey: "a".repeat(64),
    projectContextRevision: 1,
    substrateIdentity: "fixture",
    pathCount: 1,
    rootCount: 1,
    edgeKeys: new Set(["edge-abc"]),
    rootEdgeKeys: new Set(),
    memberCoordinateKeys: new Set(["goal:c", "requirement:a", "resource:b"]),
    routeCoordinateKeys: new Set(["goal:c", "requirement:a"]),
    rootCoordinateKeys: new Set(["requirement:a"]),
    terminalCoordinateKeys: new Set(["goal:c"]),
    relationDocumentIdsByEdge: new Map([
      ["edge-abc", new Set(["context-abc"])],
    ]),
    rootRelationDocumentIdsByEdge: new Map(),
    boundsTargetIds: [
      "coordinate:goal:c",
      "coordinate:requirement:a",
      "coordinate:resource:b",
      "edge-hub:edge-abc",
    ],
    ...overrides,
  };
}

function semanticByKey(elements, kind) {
  return Object.fromEntries(
    [...elements.nodes, ...elements.edges]
      .filter((element) => element.data?.kind === kind)
      .map((element) => {
        const data = element.data;
        if (kind === "coordinate") {
          return [data.coordinate.coordinateKey, data.semanticEmphasis];
        }
        if (kind === "hub") {
          return [data.hub.edgeKey, data.semanticEmphasis];
        }
        return [element.id, data.semanticEmphasis];
      }),
  );
}

test("no target leaves every graph element in its normal presentation", () => {
  const { graph, layout } = fixture();
  const elements = buildProjectContextFlowElements(graph, layout, null);
  for (const element of [...elements.nodes, ...elements.edges]) {
    if (element.data?.kind !== "island") {
      assert.equal(element.data?.emphasis, "normal");
      assert.equal(element.data?.semanticEmphasis, "none");
    }
  }
  for (const edge of elements.edges) {
    assert.equal(edge.focusable, false);
    assert.equal(edge.domAttributes?.["aria-hidden"], true);
  }
});

test("semantic path marks one complete Hyperedge without lighting an overlap Edge", () => {
  const { graph, layout } = fixture();
  const elements = buildProjectContextFlowElements(
    graph,
    layout,
    null,
    semanticOverlay(),
  );

  assert.deepEqual(semanticByKey(elements, "hub"), {
    "edge-ab": "outside",
    "edge-abc": "route",
  });
  assert.deepEqual(semanticByKey(elements, "coordinate"), {
    "goal:c": "route",
    "requirement:a": "route",
    "resource:b": "member",
  });
  for (const edge of elements.edges) {
    assert.equal(
      edge.data?.semanticEmphasis,
      edge.data?.edgeKey === "edge-abc" ? "member" : "outside",
    );
  }

  const coordinates = elements.nodes.filter(
    (node) => node.data.kind === "coordinate",
  );
  assert.equal(
    coordinates.find(
      (node) => node.data.coordinate.coordinateKey === "requirement:a",
    )?.data.semanticRoot,
    true,
  );
  assert.equal(
    coordinates.find((node) => node.data.coordinate.coordinateKey === "goal:c")
      ?.data.semanticTerminal,
    true,
  );
});

test("selection remains an independent axis for an item outside the semantic path", () => {
  const { graph, layout } = fixture();
  const elements = buildProjectContextFlowElements(
    graph,
    layout,
    { kind: "edge", key: "edge-ab" },
    semanticOverlay(),
  );

  const hubs = elements.nodes.filter((node) => node.data.kind === "hub");
  const selectedOutside = hubs.find(
    (node) => node.data.hub.edgeKey === "edge-ab",
  );
  const semanticRoute = hubs.find(
    (node) => node.data.hub.edgeKey === "edge-abc",
  );
  assert.equal(selectedOutside?.data.emphasis, "active");
  assert.equal(selectedOutside?.data.semanticEmphasis, "outside");
  assert.equal(semanticRoute?.data.emphasis, "dimmed");
  assert.equal(semanticRoute?.data.semanticEmphasis, "route");
});

test("zero-hop roots get markers without inventing a traversed Hyperedge", () => {
  const { graph, layout } = fixture();
  const overlay = semanticOverlay({
    edgeKeys: new Set(),
    rootEdgeKeys: new Set(["edge-ab"]),
    memberCoordinateKeys: new Set(),
    routeCoordinateKeys: new Set(),
    rootCoordinateKeys: new Set(["resource:b"]),
    terminalCoordinateKeys: new Set(),
    pathCount: 0,
  });
  const elements = buildProjectContextFlowElements(
    graph,
    layout,
    null,
    overlay,
  );

  assert.deepEqual(semanticByKey(elements, "hub"), {
    "edge-ab": "member",
    "edge-abc": "outside",
  });
  assert.deepEqual(semanticByKey(elements, "coordinate"), {
    "goal:c": "outside",
    "requirement:a": "outside",
    "resource:b": "route",
  });
  for (const edge of elements.edges) {
    assert.equal(edge.data?.semanticEmphasis, "outside");
  }
});

test("a valid empty semantic result does not dim the canonical graph", () => {
  const { graph, layout } = fixture();
  const overlay = semanticOverlay({
    edgeKeys: new Set(),
    rootEdgeKeys: new Set(),
    memberCoordinateKeys: new Set(),
    routeCoordinateKeys: new Set(),
    rootCoordinateKeys: new Set(),
    terminalCoordinateKeys: new Set(),
    boundsTargetIds: [],
    pathCount: 0,
    rootCount: 0,
  });
  const elements = buildProjectContextFlowElements(
    graph,
    layout,
    null,
    overlay,
  );
  for (const element of [...elements.nodes, ...elements.edges]) {
    if (element.data?.kind !== "island") {
      assert.equal(element.data?.semanticEmphasis, "none");
    }
  }
});

test("Edge focus highlights one exact Hub, all of its Spokes, and no overlap Edge", () => {
  const { graph, layout } = fixture();
  const elements = buildProjectContextFlowElements(graph, layout, {
    kind: "edge",
    key: "edge-abc",
  });

  assert.deepEqual(emphasisByKey(elements, "hub"), {
    "edge-ab": "dimmed",
    "edge-abc": "active",
  });
  assert.deepEqual(emphasisByKey(elements, "coordinate"), {
    "goal:c": "active",
    "requirement:a": "active",
    "resource:b": "active",
  });
  for (const edge of elements.edges) {
    assert.equal(
      edge.data?.emphasis,
      edge.data?.edgeKey === "edge-abc" ? "active" : "dimmed",
    );
  }
});

test("Coordinate focus highlights its incident Hubs but only its own Spokes", () => {
  const { graph, layout } = fixture();
  const elements = buildProjectContextFlowElements(graph, layout, {
    kind: "coordinate",
    key: "requirement:a",
  });

  assert.deepEqual(emphasisByKey(elements, "hub"), {
    "edge-ab": "active",
    "edge-abc": "active",
  });
  assert.deepEqual(emphasisByKey(elements, "coordinate"), {
    "goal:c": "dimmed",
    "requirement:a": "active",
    "resource:b": "dimmed",
  });
  for (const edge of elements.edges) {
    assert.equal(
      edge.data?.emphasis,
      edge.data?.coordinateKey === "requirement:a" ? "active" : "dimmed",
    );
  }
});

test("Island hues stay stable and avoid immediate palette repetition", () => {
  assert.equal(projectContextIslandHue(1), 267);
  assert.equal(projectContextIslandHue(2), 196);
  assert.equal(projectContextIslandHue(9), 290);
  assert.equal(
    new Set(
      Array.from({ length: 16 }, (_, index) =>
        projectContextIslandHue(index + 1),
      ),
    ).size,
    16,
  );
});

test("focused Coordinates carry a non-domain Query Anchor presentation flag", () => {
  const objectId = "60000000-0000-4000-8000-000000000001";
  const graph = buildProjectContextGraph({
    communityKey: "community-0",
    projectId: "project",
    relayPubkey: "a".repeat(64),
    context: {
      contextRevision: 1,
      projectionGeneration: 1,
      activeEdgeCount: 1,
      boundDocumentCount: 1,
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
    projectViewObservation: { state: "observed" },
    documentObservation: { state: "observed" },
    edges: [],
    coordinateDetails: [
      {
        coordinateKey: `requirement:${objectId}`,
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId,
        },
        state: "active",
        title: "Anchor",
      },
    ],
    documentDetails: [],
  });
  const elements = buildProjectContextFlowElements(
    graph,
    layoutProjectContextGraph(graph),
    null,
  );

  assert.equal(elements.nodes.length, 1);
  assert.equal(elements.nodes[0].data.kind, "coordinate");
  assert.equal(elements.nodes[0].data.queryAnchor, true);
});
