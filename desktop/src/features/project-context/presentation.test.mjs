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

test("no target leaves every graph element in its normal presentation", () => {
  const { graph, layout } = fixture();
  const elements = buildProjectContextFlowElements(graph, layout, null);
  for (const element of [...elements.nodes, ...elements.edges]) {
    if (element.data?.kind !== "island") {
      assert.equal(element.data?.emphasis, "normal");
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

test("Island hues are stable and wrap without becoming domain identity", () => {
  assert.equal(projectContextIslandHue(1), 267);
  assert.equal(projectContextIslandHue(2), 196);
  assert.equal(projectContextIslandHue(9), 267);
});
