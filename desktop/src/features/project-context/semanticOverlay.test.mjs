import assert from "node:assert/strict";
import test from "node:test";

import {
  buildProjectContextSemanticOverlay,
  projectContextSemanticSubstrateIdentity,
  semanticOverlayMatchesSubstrate,
} from "./semanticOverlay.ts";

function substrate() {
  const coordinateKeys = ["requirement:a", "work:b", "issue:c", "goal:d"];
  return {
    communityKey: "community-0",
    projectId: "project-0",
    relayPubkey: "a".repeat(64),
    context: {
      contextRevision: 7,
      projectionGeneration: 2,
      activeEdgeCount: 2,
      boundDocumentCount: 3,
      updatedAt: "2026-08-11T00:00:00Z",
      metaEventId: "b".repeat(64),
      capabilityEnabled: true,
    },
    query: { type: "contains_all", coordinates: [] },
    projectViewObservation: { state: "observed" },
    documentObservation: { state: "observed" },
    meetingObservations: [],
    edges: [
      {
        edgeKey: "edge-abc",
        coordinateKeys: coordinateKeys.slice(0, 3),
        contextDocumentIds: ["document-1", "document-2"],
      },
      {
        edgeKey: "edge-cd",
        coordinateKeys: coordinateKeys.slice(2),
        contextDocumentIds: ["document-3"],
      },
    ],
    coordinateDetails: coordinateKeys.map((coordinateKey) => ({
      coordinateKey,
      coordinate: {
        type: "project_view_object",
        objectType: coordinateKey.split(":")[0],
        objectId: coordinateKey.split(":")[1],
      },
      state: "active",
      title: coordinateKey,
    })),
    documentDetails: [],
  };
}

function semanticResult() {
  return {
    communityKey: "community-0",
    requestId: "request-0",
    projectId: "project-0",
    relayPubkey: "a".repeat(64),
    projectContextRevision: 7,
    roots: [
      {
        rootId: "root-1",
        coordinateEntrypoints: ["requirement:a"],
        contextDocumentEntrypoints: [
          { edgeKey: "edge-cd", documentId: "document-3" },
        ],
      },
      {
        rootId: "root-2",
        coordinateEntrypoints: ["work:b"],
        contextDocumentEntrypoints: [],
      },
    ],
    paths: [
      {
        pathId: "path-1",
        rootId: "root-1",
        hops: [
          {
            ordinal: 1,
            edgeKey: "edge-abc",
            completeCoordinateKeys: ["requirement:a", "work:b", "issue:c"],
            currentContextDocumentIds: ["document-1", "document-2"],
            enteredFromCoordinateKey: "requirement:a",
            selectedContextDocumentId: "document-1",
            continuedToCoordinateKey: "issue:c",
          },
          {
            ordinal: 2,
            edgeKey: "edge-cd",
            completeCoordinateKeys: ["issue:c", "goal:d"],
            currentContextDocumentIds: ["document-3"],
            enteredFromCoordinateKey: "issue:c",
            selectedContextDocumentId: "document-3",
            continuedToCoordinateKey: "goal:d",
          },
        ],
      },
      {
        pathId: "path-2",
        rootId: "root-2",
        hops: [
          {
            ordinal: 1,
            edgeKey: "edge-abc",
            completeCoordinateKeys: ["requirement:a", "work:b", "issue:c"],
            currentContextDocumentIds: ["document-2", "document-1"],
            enteredFromCoordinateKey: "work:b",
            selectedContextDocumentId: "document-2",
            continuedToCoordinateKey: "requirement:a",
          },
        ],
      },
    ],
  };
}

function clone(value) {
  return structuredClone(value);
}

test("atomically builds one complete Hyperedge union with roots and path terminals", () => {
  const graph = substrate();
  const result = semanticResult();
  const graphBefore = clone(graph);
  const resultBefore = clone(result);
  const built = buildProjectContextSemanticOverlay(result, graph);

  assert.equal(built.ok, true);
  const overlay = built.overlay;
  assert.deepEqual([...overlay.edgeKeys].sort(), ["edge-abc", "edge-cd"]);
  assert.deepEqual([...overlay.rootEdgeKeys], ["edge-cd"]);
  assert.deepEqual([...overlay.memberCoordinateKeys].sort(), [
    "goal:d",
    "issue:c",
    "requirement:a",
    "work:b",
  ]);
  assert.deepEqual([...overlay.routeCoordinateKeys].sort(), [
    "goal:d",
    "issue:c",
    "requirement:a",
    "work:b",
  ]);
  assert.deepEqual([...overlay.rootCoordinateKeys].sort(), [
    "requirement:a",
    "work:b",
  ]);
  assert.deepEqual([...overlay.terminalCoordinateKeys].sort(), [
    "goal:d",
    "requirement:a",
  ]);
  assert.deepEqual(
    [...overlay.relationDocumentIdsByEdge.get("edge-abc")].sort(),
    ["document-1", "document-2"],
  );
  assert.deepEqual(
    [...overlay.rootRelationDocumentIdsByEdge.get("edge-cd")],
    ["document-3"],
  );
  assert.deepEqual(overlay.boundsTargetIds, [
    "coordinate:goal:d",
    "coordinate:issue:c",
    "coordinate:requirement:a",
    "coordinate:work:b",
    "edge-hub:edge-abc",
    "edge-hub:edge-cd",
  ]);
  assert.deepEqual(graph, graphBefore);
  assert.deepEqual(result, resultBefore);
});

test("retains valid zero-hop Coordinate and Context Document roots", () => {
  const result = semanticResult();
  result.paths = [];
  const built = buildProjectContextSemanticOverlay(result, substrate());

  assert.equal(built.ok, true);
  assert.equal(built.overlay.pathCount, 0);
  assert.deepEqual([...built.overlay.edgeKeys], []);
  assert.deepEqual([...built.overlay.rootCoordinateKeys].sort(), [
    "requirement:a",
    "work:b",
  ]);
  assert.deepEqual([...built.overlay.rootEdgeKeys], ["edge-cd"]);
  assert.deepEqual(
    [...built.overlay.rootRelationDocumentIdsByEdge.get("edge-cd")],
    ["document-3"],
  );
});

test("fails the whole overlay when any Coordinate structural root is missing", () => {
  const result = semanticResult();
  result.roots[0].coordinateEntrypoints = ["resource:missing"];
  assert.deepEqual(buildProjectContextSemanticOverlay(result, substrate()), {
    ok: false,
    reason: "missing_root",
  });
});

test("fails the whole overlay when a Context Document root is not currently bound", () => {
  const result = semanticResult();
  result.roots[0].contextDocumentEntrypoints[0].documentId = "document-missing";
  assert.deepEqual(buildProjectContextSemanticOverlay(result, substrate()), {
    ok: false,
    reason: "missing_root_document",
  });
});

test("rejects missing Edges and incomplete Coordinate or binding sets", () => {
  const missingEdge = semanticResult();
  missingEdge.paths[0].hops[0].edgeKey = "edge-missing";
  assert.equal(
    buildProjectContextSemanticOverlay(missingEdge, substrate()).reason,
    "missing_edge",
  );

  const missingCoordinate = semanticResult();
  missingCoordinate.paths[0].hops[0].completeCoordinateKeys.pop();
  assert.equal(
    buildProjectContextSemanticOverlay(missingCoordinate, substrate()).reason,
    "coordinate_set_mismatch",
  );

  const extraBinding = semanticResult();
  extraBinding.paths[0].hops[0].currentContextDocumentIds.push("document-x");
  assert.equal(
    buildProjectContextSemanticOverlay(extraBinding, substrate()).reason,
    "binding_set_mismatch",
  );
});

test("rejects a selected Document or route Coordinate outside its complete Edge", () => {
  const selectedDocument = semanticResult();
  selectedDocument.paths[0].hops[0].selectedContextDocumentId = "document-3";
  assert.equal(
    buildProjectContextSemanticOverlay(selectedDocument, substrate()).reason,
    "selected_document_mismatch",
  );

  const routeCoordinate = semanticResult();
  routeCoordinate.paths[0].hops[0].continuedToCoordinateKey = "goal:d";
  assert.equal(
    buildProjectContextSemanticOverlay(routeCoordinate, substrate()).reason,
    "route_coordinate_mismatch",
  );
});

test("requires exact identity, revision, and an All Context substrate", () => {
  const focused = substrate();
  focused.query = {
    type: "incident",
    coordinate: {
      type: "project_view_object",
      objectType: "requirement",
      objectId: "a",
    },
  };
  assert.equal(
    buildProjectContextSemanticOverlay(semanticResult(), focused).reason,
    "not_all_context",
  );

  const wrongIdentity = semanticResult();
  wrongIdentity.projectId = "other-project";
  assert.equal(
    buildProjectContextSemanticOverlay(wrongIdentity, substrate()).reason,
    "identity_mismatch",
  );

  const wrongRevision = semanticResult();
  wrongRevision.projectContextRevision = 8;
  assert.equal(
    buildProjectContextSemanticOverlay(wrongRevision, substrate()).reason,
    "revision_mismatch",
  );
});

test("render-time identity gate rejects revision, Edge, and binding changes", () => {
  const graph = substrate();
  const built = buildProjectContextSemanticOverlay(semanticResult(), graph);
  assert.equal(built.ok, true);
  assert.equal(semanticOverlayMatchesSubstrate(built.overlay, graph), true);

  const revisionChanged = clone(graph);
  revisionChanged.context.contextRevision += 1;
  assert.equal(
    semanticOverlayMatchesSubstrate(built.overlay, revisionChanged),
    false,
  );

  const bindingChanged = clone(graph);
  bindingChanged.edges[0].contextDocumentIds.push("document-4");
  assert.equal(
    semanticOverlayMatchesSubstrate(built.overlay, bindingChanged),
    false,
  );

  const membershipChanged = clone(graph);
  membershipChanged.edges[0].coordinateKeys.pop();
  assert.equal(
    semanticOverlayMatchesSubstrate(built.overlay, membershipChanged),
    false,
  );

  const focused = clone(graph);
  focused.query = {
    type: "incident",
    coordinate: {
      type: "project_view_object",
      objectType: "requirement",
      objectId: "a",
    },
  };
  assert.equal(semanticOverlayMatchesSubstrate(built.overlay, focused), false);
});

test("substrate identity and overlay output are deterministic across canonical ordering", () => {
  const graph = substrate();
  const reordered = clone(graph);
  reordered.edges.reverse();
  reordered.edges[0].coordinateKeys.reverse();
  reordered.edges[1].contextDocumentIds.reverse();
  assert.equal(
    projectContextSemanticSubstrateIdentity(graph),
    projectContextSemanticSubstrateIdentity(reordered),
  );

  const first = buildProjectContextSemanticOverlay(semanticResult(), graph);
  const second = buildProjectContextSemanticOverlay(
    semanticResult(),
    reordered,
  );
  assert.equal(first.ok, true);
  assert.equal(second.ok, true);
  assert.deepEqual(first.overlay, second.overlay);
});

test("maps a bounded semantic result against a 1000-Edge substrate", () => {
  const graph = substrate();
  graph.context.activeEdgeCount = 1_000;
  graph.context.boundDocumentCount = 1_000;
  graph.edges = Array.from({ length: 1_000 }, (_, index) => ({
    edgeKey: `edge-${index.toString().padStart(4, "0")}`,
    coordinateKeys: [`work:${index}`, `issue:${index}`],
    contextDocumentIds: [`document-${index}`],
  }));
  const result = {
    ...semanticResult(),
    roots: [
      {
        rootId: "root-large",
        coordinateEntrypoints: ["work:999"],
        contextDocumentEntrypoints: [],
      },
    ],
    paths: [
      {
        pathId: "path-large",
        rootId: "root-large",
        hops: [
          {
            ordinal: 1,
            edgeKey: "edge-0999",
            completeCoordinateKeys: ["work:999", "issue:999"],
            currentContextDocumentIds: ["document-999"],
            enteredFromCoordinateKey: "work:999",
            selectedContextDocumentId: "document-999",
            continuedToCoordinateKey: "issue:999",
          },
        ],
      },
    ],
  };

  const built = buildProjectContextSemanticOverlay(result, graph);
  assert.equal(built.ok, true);
  assert.deepEqual([...built.overlay.edgeKeys], ["edge-0999"]);
  assert.deepEqual([...built.overlay.memberCoordinateKeys].sort(), [
    "issue:999",
    "work:999",
  ]);
  assert.equal(semanticOverlayMatchesSubstrate(built.overlay, graph), true);
});
