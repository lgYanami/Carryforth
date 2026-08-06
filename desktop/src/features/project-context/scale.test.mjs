import assert from "node:assert/strict";
import test from "node:test";

import { buildProjectContextGraph } from "./graph.ts";
import { layoutProjectContextGraph } from "./layout.ts";
import { buildProjectContextFlowElements } from "./presentation.ts";

function largeResult(edgeCount) {
  const edges = [];
  const coordinateDetails = [];
  for (let index = 0; index < edgeCount; index += 1) {
    const suffix = String(index).padStart(6, "0");
    const requirementKey = `requirement:req-${suffix}`;
    const resourceKey = `resource:res-${suffix}`;
    edges.push({
      edgeKey: `edge-${suffix}`,
      coordinateKeys: [requirementKey, resourceKey],
      contextDocumentIds: [`document-${suffix}`],
    });
    coordinateDetails.push(
      {
        coordinateKey: requirementKey,
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId: `req-${suffix}`,
        },
        state: "active",
        title: `Requirement ${suffix}`,
      },
      {
        coordinateKey: resourceKey,
        coordinate: {
          type: "project_view_object",
          objectType: "resource",
          objectId: `res-${suffix}`,
        },
        state: "active",
        title: `Resource ${suffix}`,
      },
    );
  }
  return {
    communityKey: "community-scale",
    projectId: "project-scale",
    relayPubkey: "a".repeat(64),
    context: {
      contextRevision: 1,
      projectionGeneration: 1,
      activeEdgeCount: edgeCount,
      boundDocumentCount: edgeCount,
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

for (const edgeCount of [100, 500, 1_000]) {
  test(`${edgeCount} Edge result remains complete through graph, layout, and presentation`, () => {
    const graph = buildProjectContextGraph(largeResult(edgeCount));
    const layout = layoutProjectContextGraph(graph);
    const elements = buildProjectContextFlowElements(graph, layout, null);

    assert.equal(graph.hubs.length, edgeCount);
    assert.equal(graph.coordinates.length, edgeCount * 2);
    assert.equal(graph.spokes.length, edgeCount * 2);
    assert.equal(graph.islands.length, edgeCount);
    assert.equal(layout.nodes.length, edgeCount * 3);
    assert.equal(layout.spokes.length, edgeCount * 2);
    assert.equal(elements.nodes.length, edgeCount * 4);
    assert.equal(elements.edges.length, edgeCount * 2);
    assert.deepEqual(layoutProjectContextGraph(graph), layout);
  });
}

test("selection discovery stays complete in a 1000 Edge result", () => {
  const graph = buildProjectContextGraph(largeResult(1_000));
  const layout = layoutProjectContextGraph(graph);
  const selected = buildProjectContextFlowElements(graph, layout, {
    kind: "edge",
    key: "edge-000999",
  });
  assert.equal(
    selected.nodes.filter(
      (node) => node.data.kind !== "island" && node.data.emphasis === "active",
    ).length,
    3,
  );
  assert.equal(
    selected.edges.filter((edge) => edge.data?.emphasis === "active").length,
    2,
  );
});
