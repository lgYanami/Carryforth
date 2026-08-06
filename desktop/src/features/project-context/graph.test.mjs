import assert from "node:assert/strict";
import test from "node:test";

import {
  buildProjectContextGraph,
  projectContextCoordinateNodeId,
  projectContextHubNodeId,
  projectContextSpokeId,
} from "./graph.ts";

function projectViewDetail(key, objectType, state = "active") {
  const objectId = key.slice(key.indexOf(":") + 1);
  return {
    coordinateKey: key,
    coordinate: {
      type: "project_view_object",
      objectType,
      objectId,
    },
    state,
    title: `${objectType} ${objectId}`,
  };
}

function documentDetail(key, state = "active") {
  const documentId = key.slice(key.indexOf(":") + 1);
  return {
    coordinateKey: key,
    coordinate: { type: "document", documentId },
    state,
    title: `Document ${documentId}`,
  };
}

function result(edges, coordinateDetails) {
  return {
    communityKey: "community-0",
    projectId: "project",
    relayPubkey: "a".repeat(64),
    context: {
      contextRevision: 1,
      projectionGeneration: 1,
      activeEdgeCount: edges.length,
      boundDocumentCount: edges.reduce(
        (count, edge) => count + edge.contextDocumentIds.length,
        0,
      ),
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
  };
}

const A = "requirement:a";
const B = "resource:b";
const C = "goal:c";
const D = "document:d";

test("canonical ids preserve one Coordinate, Hub, and Spoke identity", () => {
  assert.equal(projectContextCoordinateNodeId(A), `coordinate:${A}`);
  assert.equal(projectContextHubNodeId("edge-ab"), "edge-hub:edge-ab");
  assert.equal(projectContextSpokeId("edge-ab", A), `spoke:edge-ab:${A}`);
});

test("binary Edge becomes one Hub and two undirected presentation Spokes", () => {
  const graph = buildProjectContextGraph(
    result(
      [
        {
          edgeKey: "edge-ab",
          coordinateKeys: [B, A],
          contextDocumentIds: ["context-1"],
        },
      ],
      [projectViewDetail(A, "requirement"), projectViewDetail(B, "resource")],
    ),
  );

  assert.deepEqual(
    graph.coordinates.map((coordinate) => coordinate.coordinateKey),
    [A, B],
  );
  assert.equal(graph.hubs.length, 1);
  assert.equal(graph.spokes.length, 2);
  assert.equal(graph.islands.length, 1);
  assert.deepEqual(graph.islands[0].contextDocumentIds, ["context-1"]);
  assert.equal(
    graph.coordinates.some((coordinate) => coordinate.stableId === "context-1"),
    false,
  );
});

test("hyperedge remains one Hub with all three Coordinate incidences", () => {
  const graph = buildProjectContextGraph(
    result(
      [
        {
          edgeKey: "edge-abc",
          coordinateKeys: [A, B, C],
          contextDocumentIds: ["context-2", "context-3"],
        },
      ],
      [
        projectViewDetail(A, "requirement"),
        projectViewDetail(B, "resource"),
        projectViewDetail(C, "goal"),
      ],
    ),
  );

  assert.equal(graph.hubs.length, 1);
  assert.deepEqual(graph.hubs[0].coordinateKeys, [C, A, B]);
  assert.equal(graph.spokes.length, 3);
  assert.equal(graph.islands[0].coordinateKeys.length, 3);
});

test("AB and ABC overlap without merging either domain Edge", () => {
  const graph = buildProjectContextGraph(
    result(
      [
        {
          edgeKey: "edge-ab",
          coordinateKeys: [A, B],
          contextDocumentIds: ["context-ab"],
        },
        {
          edgeKey: "edge-abc",
          coordinateKeys: [A, B, C],
          contextDocumentIds: ["context-abc"],
        },
      ],
      [
        projectViewDetail(A, "requirement"),
        projectViewDetail(B, "resource"),
        projectViewDetail(C, "goal"),
      ],
    ),
  );

  assert.equal(graph.coordinates.length, 3);
  assert.equal(graph.hubs.length, 2);
  assert.equal(graph.spokes.length, 5);
  assert.equal(graph.islands.length, 1);
  assert.deepEqual(graph.islands[0].edgeKeys, ["edge-ab", "edge-abc"]);
});

test("shared Coordinates merge Islands while disjoint Edge sets stay separate", () => {
  const graph = buildProjectContextGraph(
    result(
      [
        {
          edgeKey: "edge-ab",
          coordinateKeys: [A, B],
          contextDocumentIds: ["context-ab"],
        },
        {
          edgeKey: "edge-bc",
          coordinateKeys: [B, C],
          contextDocumentIds: ["context-bc"],
        },
        {
          edgeKey: "edge-dx",
          coordinateKeys: [D, "work:x"],
          contextDocumentIds: ["context-dx"],
        },
      ],
      [
        projectViewDetail(A, "requirement"),
        projectViewDetail(B, "resource"),
        projectViewDetail(C, "goal"),
        documentDetail(D),
        projectViewDetail("work:x", "work"),
      ],
    ),
  );

  assert.equal(graph.islands.length, 2);
  assert.deepEqual(graph.islands[0].edgeKeys, ["edge-ab", "edge-bc"]);
  assert.deepEqual(graph.islands[1].edgeKeys, ["edge-dx"]);
  assert.deepEqual(
    graph.islands.map((island) => island.index),
    [1, 2],
  );
});

test("a Context Document binding connects nothing unless it is an explicit Coordinate", () => {
  const graph = buildProjectContextGraph(
    result(
      [
        {
          edgeKey: "edge-ab",
          coordinateKeys: [A, B],
          contextDocumentIds: ["d"],
        },
        {
          edgeKey: "edge-cd",
          coordinateKeys: [C, D],
          contextDocumentIds: ["context-cd"],
        },
      ],
      [
        projectViewDetail(A, "requirement"),
        projectViewDetail(B, "resource"),
        projectViewDetail(C, "goal"),
        documentDetail(D),
      ],
    ),
  );

  assert.equal(graph.islands.length, 2);
  assert.equal(
    graph.coordinates.filter((coordinate) => coordinate.stableId === "d")
      .length,
    1,
  );
});

test("tombstoned and unavailable Coordinates remain structural members", () => {
  const graph = buildProjectContextGraph(
    result(
      [
        {
          edgeKey: "edge-ab",
          coordinateKeys: [A, B],
          contextDocumentIds: ["context-ab"],
        },
      ],
      [
        projectViewDetail(A, "requirement", "tombstoned"),
        projectViewDetail(B, "resource", "unavailable"),
      ],
    ),
  );

  assert.equal(graph.islands.length, 1);
  assert.deepEqual(
    graph.coordinates.map((coordinate) => coordinate.state),
    ["tombstoned", "unavailable"],
  );
});

test("shuffled trusted arrays produce the same canonical graph", () => {
  const canonicalResult = result(
    [
      {
        edgeKey: "edge-ab",
        coordinateKeys: [A, B],
        contextDocumentIds: ["context-b", "context-a"],
      },
      {
        edgeKey: "edge-cd",
        coordinateKeys: [C, D],
        contextDocumentIds: ["context-d"],
      },
    ],
    [
      projectViewDetail(A, "requirement"),
      projectViewDetail(B, "resource"),
      projectViewDetail(C, "goal"),
      documentDetail(D),
    ],
  );
  const shuffled = structuredClone(canonicalResult);
  shuffled.edges.reverse();
  shuffled.edges.forEach((edge) => {
    edge.coordinateKeys.reverse();
  });
  shuffled.coordinateDetails.reverse();

  assert.deepEqual(
    buildProjectContextGraph(shuffled),
    buildProjectContextGraph(canonicalResult),
  );
});
