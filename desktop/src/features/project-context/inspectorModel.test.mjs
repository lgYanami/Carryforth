import assert from "node:assert/strict";
import test from "node:test";

import {
  firstReadableProjectContextDocumentId,
  projectContextDocumentIdentity,
  projectContextIncidentEdgeKeys,
  projectContextInspectedCoordinate,
  projectContextInspectedEdge,
  projectContextProjectViewObject,
  projectContextProjectViewRelations,
} from "./inspectorModel.ts";

const PROJECT_ID = "10000000-0000-4000-8000-000000000001";
const REQUIREMENT_ID = "20000000-0000-4000-8000-000000000001";
const PLAN_ID = "30000000-0000-4000-8000-000000000001";
const DOCUMENT_ID = "40000000-0000-4000-8000-000000000001";
const RELAY = "b".repeat(64);

function result() {
  return {
    communityKey: "community-a:0",
    projectId: PROJECT_ID,
    relayPubkey: RELAY,
    context: {
      contextRevision: 3,
      projectionGeneration: 1,
      activeEdgeCount: 2,
      boundDocumentCount: 2,
      updatedAt: "2026-08-06T08:00:00Z",
      metaEventId: "c".repeat(64),
      capabilityEnabled: true,
    },
    query: { type: "contains_all", coordinates: [] },
    projectViewObservation: {
      state: "observed",
      projectRevision: 8,
      projectionGeneration: 2,
    },
    documentObservation: {
      state: "observed",
      catalogRevision: 5,
      projectionGeneration: 4,
    },
    meetingObservations: [],
    edges: [
      {
        edgeKey: "2".repeat(64),
        coordinateKeys: [
          `document:${DOCUMENT_ID}`,
          `requirement:${REQUIREMENT_ID}`,
        ],
        contextDocumentIds: [DOCUMENT_ID],
      },
      {
        edgeKey: "1".repeat(64),
        coordinateKeys: [`plan:${PLAN_ID}`, `requirement:${REQUIREMENT_ID}`],
        contextDocumentIds: ["50000000-0000-4000-8000-000000000001"],
      },
    ],
    coordinateDetails: [
      {
        coordinateKey: `requirement:${REQUIREMENT_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId: REQUIREMENT_ID,
        },
        state: "active",
        title: "Verified requirement",
      },
      {
        coordinateKey: `document:${DOCUMENT_ID}`,
        coordinate: { type: "document", documentId: DOCUMENT_ID },
        state: "active",
        title: "Dual-role document",
      },
    ],
    documentDetails: [
      {
        documentId: DOCUMENT_ID,
        state: "active",
        title: "Bound rationale",
        documentRevision: 3,
      },
    ],
  };
}

function object(objectType, id, relations = {}) {
  const titleField =
    objectType === "plan" ? { title: "Plan" } : { title: "Requirement" };
  return {
    id,
    objectType,
    objectRevision: 2,
    projectRevision: 8,
    createdAt: "2026-08-06T07:00:00Z",
    updatedAt: "2026-08-06T08:00:00Z",
    createdBy: "a".repeat(64),
    updatedBy: "a".repeat(64),
    data: {
      ...titleField,
      description: "Verified body",
      status: "active",
      priority: "high",
    },
    relations,
  };
}

function viewResult() {
  const requirement = object("requirement", REQUIREMENT_ID, {
    underPlanId: PLAN_ID,
  });
  const plan = object("plan", PLAN_ID);
  return {
    status: "ready",
    relayPubkey: RELAY,
    contextCapability: false,
    schemaVersion: 3,
    projectRevision: 8,
    projectionGeneration: 2,
    activeObjectCount: 3,
    updatedAt: "2026-08-06T08:00:00Z",
    view: {
      profile: {
        ...object("project_profile", "60000000-0000-4000-8000-000000000001"),
        data: {
          name: "Project",
          positioning: "Positioning",
          purpose: "Purpose",
          problem: "Problem",
          scope: "Scope",
        },
      },
      goals: [],
      unboundPlans: [
        {
          plan,
          stages: [],
        },
      ],
      unplannedRequirements: [{ requirement, works: [] }],
      unplannedIssues: [],
      roles: [],
      resources: [],
      issueReferencesByTarget: {},
    },
  };
}

test("Edge inspection keeps full sets and preserves Document coordinate/content roles", () => {
  const inspected = projectContextInspectedEdge(result(), "2".repeat(64));
  assert.ok(inspected);
  assert.deepEqual(
    inspected.coordinates.map((detail) => detail.coordinateKey),
    [`document:${DOCUMENT_ID}`, `requirement:${REQUIREMENT_ID}`],
  );
  assert.equal(inspected.documents[0].documentId, DOCUMENT_ID);
  assert.equal(inspected.documents[0].title, "Bound rationale");
  assert.equal(
    inspected.coordinates[0].title,
    "Dual-role document",
    "coordinate presentation stays independent from binding presentation",
  );
});

test("missing hydrated rows become unavailable without erasing topology", () => {
  const inspected = projectContextInspectedEdge(result(), "1".repeat(64));
  assert.ok(inspected);
  assert.equal(inspected.coordinates[0].state, "unavailable");
  assert.equal(inspected.documents[0].state, "unavailable");
  assert.equal(
    projectContextInspectedCoordinate(result(), `plan:${PLAN_ID}`).coordinate
      .objectId,
    PLAN_ID,
  );
});

test("Document identity requires an observed source and selects the first active body", () => {
  const trusted = result();
  assert.deepEqual(projectContextDocumentIdentity(trusted), {
    communityKey: "community-a:0",
    projectId: PROJECT_ID,
    relayPubkey: RELAY,
    projectionGeneration: 4,
  });
  assert.equal(
    firstReadableProjectContextDocumentId(
      [
        { documentId: "deleted", state: "tombstoned" },
        { documentId: DOCUMENT_ID, state: "active" },
      ],
      projectContextDocumentIdentity(trusted),
    ),
    DOCUMENT_ID,
  );
  trusted.documentObservation = { state: "unavailable" };
  assert.equal(projectContextDocumentIdentity(trusted), undefined);
  assert.equal(
    firstReadableProjectContextDocumentId(
      [{ documentId: DOCUMENT_ID, state: "active" }],
      undefined,
    ),
    undefined,
  );
});

test("Coordinate membership is stable and includes every matching hyperedge", () => {
  assert.deepEqual(
    projectContextIncidentEdgeKeys(result(), `requirement:${REQUIREMENT_ID}`),
    ["1".repeat(64), "2".repeat(64)],
  );
});

test("Project View detail resolves only against the matching verified generation", () => {
  const context = result();
  const detail = context.coordinateDetails[0];
  const ready = viewResult();
  const resolved = projectContextProjectViewObject({
    detail,
    projectViewResult: ready,
    result: context,
  });
  assert.equal(resolved?.id, REQUIREMENT_ID);
  assert.deepEqual(
    projectContextProjectViewRelations(ready.view, resolved).map((relation) => [
      relation.direction,
      relation.label,
      relation.target.id,
    ]),
    [["outgoing", "Under plan", PLAN_ID]],
  );

  assert.equal(
    projectContextProjectViewObject({
      detail,
      projectViewResult: { ...ready, projectionGeneration: 9 },
      result: context,
    }),
    undefined,
  );
});
