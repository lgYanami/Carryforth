import assert from "node:assert/strict";
import test from "node:test";

import {
  countProjectViewFocus,
  indexProjectViewObjects,
  projectViewIncomingReferences,
  projectViewObjectPaths,
  writableProjectViewObject,
} from "./model.ts";
import {
  assembleProjectViewV3,
  normalizeProjectViewObjectV3,
  serializeProjectViewMutationIntent,
} from "../../shared/api/tauriProjectView.ts";

const actor = "a".repeat(64);
const now = "2026-07-27T08:00:00Z";

function objectV3(
  objectType,
  id,
  data,
  relations = {},
  contextReferences = [],
) {
  return {
    id,
    object_type: objectType,
    object_revision: 1,
    project_revision: 2,
    created_at: now,
    updated_at: now,
    created_by: actor,
    updated_by: actor,
    data: { object_type: objectType, data },
    relations,
    context_references: contextReferences,
  };
}

test("normalization preserves the canonical hierarchy and camel-cases typed data", () => {
  const profile = objectV3("project_profile", "profile", {
    name: "Lora",
    positioning: "Shared context",
    purpose: "Coordinate",
    problem: "Fragmentation",
    scope: "Project context",
  });
  const goal = objectV3("goal", "goal", {
    title: "Ship",
    desired_outcome: "A legible project",
    directions: ["Verify first"],
  });
  const plan = objectV3(
    "plan",
    "plan",
    {
      title: "Client",
      description: "Read the View",
      status: "active",
    },
    { under_goal_id: "goal" },
  );
  const stage = objectV3(
    "stage",
    "stage",
    {
      title: "Read",
      description: "Render",
      status: "active",
    },
    { under_plan_id: "plan" },
  );

  const view = assembleProjectViewV3([profile, goal, plan, stage]);

  assert.equal(view.goals[0].goal.data.desiredOutcome, "A legible project");
  assert.equal(indexProjectViewObjects(view).size, 4);
  assert.deepEqual(countProjectViewFocus(view), {
    activePlans: 1,
    activeStages: 1,
    openIssues: 0,
    inProgressWork: 0,
  });
  assert.equal(
    projectViewObjectPaths(view).get("stage"),
    "Ship / Client / Read",
  );
  assert.deepEqual(
    projectViewIncomingReferences(view, "plan").map(({ relation, source }) => [
      relation,
      source.id,
    ]),
    [["under plan", "stage"]],
  );
  assert.deepEqual(writableProjectViewObject(view.goals[0].plans[0].plan), {
    objectType: "plan",
    data: {
      title: "Client",
      description: "Read the View",
      status: "active",
    },
    underGoalId: "goal",
  });
});

test("incoming references include Resource Context coordinates", () => {
  const profile = objectV3("project_profile", "profile", {
    name: "Lora",
    positioning: "Shared context",
    purpose: "Coordinate",
    problem: "Fragmentation",
    scope: "Project context",
  });
  const goal = objectV3("goal", "goal", {
    title: "Ship",
    desired_outcome: "A legible project",
    directions: [],
  });
  const resource = objectV3("resource", "resource", {
    name: "Repository",
    resource_kind: "repository",
    guide_document_id: "document-1",
  });
  const issue = objectV3(
    "issue",
    "issue",
    {
      title: "Context",
      description: "Uses the repository",
      status: "open",
      priority: "normal",
    },
    {},
    [{ type: "resource", resource_id: "resource" }],
  );
  const view = assembleProjectViewV3([profile, goal, resource, issue]);

  assert.deepEqual(
    projectViewIncomingReferences(view, "resource").map(
      ({ relation, source }) => [relation, source.id],
    ),
    [["context resource", "issue"]],
  );
});

test("normalization rejects a mismatched outer and inner object type", () => {
  const invalid = objectV3("goal", "goal", {
    title: "Ship",
    desired_outcome: "A legible project",
    directions: [],
  });
  invalid.data.object_type = "issue";

  assert.throws(
    () => normalizeProjectViewObjectV3(invalid),
    /object type does not match its data/,
  );
});

test("typed Human intents serialize optional relation clears without raw event fields", () => {
  assert.deepEqual(
    serializeProjectViewMutationIntent({
      operation: "update",
      expectedProjectRevision: 7,
      objectId: "issue",
      object: {
        objectType: "issue",
        data: {
          title: "Naming",
          description: "Avoid ambiguity",
          status: "resolved",
          priority: "high",
        },
      },
    }),
    {
      operation: "update",
      expected_project_revision: 7,
      object_type: "issue",
      object_id: "issue",
      patch: {
        title: "Naming",
        description: "Avoid ambiguity",
        status: "resolved",
        priority: "high",
        planned_in_stage_id: null,
        about: null,
      },
    },
  );

  assert.deepEqual(
    serializeProjectViewMutationIntent({
      operation: "create",
      expectedProjectRevision: 8,
      object: {
        objectType: "work",
        data: {
          title: "Implement",
          description: "Build the client",
          status: "in_progress",
          priority: "normal",
        },
        handles: { objectType: "requirement", objectId: "requirement" },
      },
    }),
    {
      operation: "create",
      expected_project_revision: 8,
      object_type: "work",
      data: {
        title: "Implement",
        description: "Build the client",
        status: "in_progress",
        priority: "normal",
        handles: {
          object_type: "requirement",
          object_id: "requirement",
        },
      },
    },
  );
});

test("v3 Context coordinates round-trip and mutation replacement is canonical", () => {
  const resourceId = "11111111-1111-4111-8111-111111111111";
  const documentId = "22222222-2222-4222-8222-222222222222";
  const profile = objectV3("project_profile", "profile", {
    name: "Lora",
    positioning: "Shared context",
    purpose: "Coordinate",
    problem: "Fragmentation",
    scope: "Project context",
  });
  profile.context_references = [
    { type: "resource", resource_id: resourceId },
    {
      type: "document",
      document_id: documentId,
      mode: "pinned",
      document_revision: 10,
    },
  ];
  const normalized = assembleProjectViewV3([profile]);
  assert.deepEqual(normalized.profile.contextReferences, [
    { referenceType: "resource", resourceId },
    {
      referenceType: "document",
      documentId,
      mode: "pinned",
      documentRevision: 10,
    },
  ]);

  assert.deepEqual(
    serializeProjectViewMutationIntent({
      operation: "context",
      expectedProjectRevision: 9,
      objectType: "project_profile",
      objectId: "profile",
      contextReferences: [
        {
          referenceType: "document",
          documentId,
          mode: "pinned",
          documentRevision: 10,
        },
        {
          referenceType: "document",
          documentId,
          mode: "pinned",
          documentRevision: 2,
        },
        { referenceType: "resource", resourceId },
        { referenceType: "document", documentId, mode: "live" },
      ],
    }),
    {
      operation: "context",
      expected_project_revision: 9,
      object_type: "project_profile",
      object_id: "profile",
      context_references: [
        { type: "resource", resource_id: resourceId },
        { type: "document", document_id: documentId, mode: "live" },
        {
          type: "document",
          document_id: documentId,
          mode: "pinned",
          document_revision: 2,
        },
        {
          type: "document",
          document_id: documentId,
          mode: "pinned",
          document_revision: 10,
        },
      ],
    },
  );
});

test("v3 assembly preserves unknown Resource kinds and Guide-only writes", () => {
  const view = assembleProjectViewV3([
    objectV3("project_profile", "profile", {
      name: "Lora",
      positioning: "Shared context",
      purpose: "Coordinate",
      problem: "Fragmentation",
      scope: "Project context",
    }),
    objectV3("goal", "goal", {
      title: "Ship",
      desired_outcome: "One verified View",
      directions: [],
    }),
    objectV3("resource", "resource", {
      name: "Release console",
      resource_kind: "internal-release-console-v7",
      summary: "Coordinates release operations",
      guide_document_id: "guide-document",
    }),
  ]);

  assert.equal(
    view.resources[0].data.resourceKind,
    "internal-release-console-v7",
  );
  assert.deepEqual(
    serializeProjectViewMutationIntent({
      operation: "update",
      expectedProjectRevision: 9,
      objectId: "resource",
      object: writableProjectViewObject(view.resources[0]),
    }),
    {
      operation: "update",
      expected_project_revision: 9,
      object_type: "resource",
      object_id: "resource",
      patch: {
        name: "Release console",
        resource_kind: "internal-release-console-v7",
        guide_document_id: "guide-document",
      },
    },
  );
  assert.equal(JSON.stringify(view.resources[0]).includes("locator"), false);
});

test("Project View update summary uses explicit KEEP, SET, and CLEAR wire", () => {
  const object = {
    objectType: "issue",
    data: {
      title: "Retry ambiguity",
      description: "An accepted write may have an uncertain response",
      status: "open",
      priority: "high",
      summary: "Relevant when investigating duplicate writes",
    },
  };
  const base = {
    operation: "update",
    expectedProjectRevision: 12,
    objectId: "issue",
    object,
  };

  assert.equal(
    Object.hasOwn(serializeProjectViewMutationIntent(base).patch, "summary"),
    false,
  );
  assert.equal(
    serializeProjectViewMutationIntent({
      ...base,
      summaryPatch: "Relevant to idempotency and uncertain write recovery",
    }).patch.summary,
    "Relevant to idempotency and uncertain write recovery",
  );
  assert.equal(
    serializeProjectViewMutationIntent({ ...base, summaryPatch: null }).patch
      .summary,
    null,
  );
});
