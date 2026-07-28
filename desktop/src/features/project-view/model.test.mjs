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
  normalizeProjectView,
  normalizeProjectViewObject,
  serializeProjectViewMutationIntent,
} from "../../shared/api/tauriProjectView.ts";

const actor = "a".repeat(64);
const now = "2026-07-27T08:00:00Z";

function object(objectType, id, data, relations = {}) {
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
  };
}

test("normalization preserves the canonical hierarchy and camel-cases typed data", () => {
  const profile = object("project_profile", "profile", {
    name: "Lora",
    positioning: "Shared context",
    purpose: "Coordinate",
    problem: "Fragmentation",
    scope: "Project context",
  });
  const goal = object("goal", "goal", {
    title: "Ship",
    desired_outcome: "A legible project",
    directions: ["Verify first"],
  });
  const plan = object(
    "plan",
    "plan",
    {
      title: "Client",
      description: "Read the View",
      status: "active",
    },
    { under_goal_id: "goal" },
  );
  const stage = object(
    "stage",
    "stage",
    {
      title: "Read",
      description: "Render",
      status: "active",
    },
    { under_plan_id: "plan" },
  );

  const view = normalizeProjectView({
    profile,
    goals: [
      {
        goal,
        plans: [
          {
            plan,
            stages: [{ stage, requirements: [], issues: [] }],
          },
        ],
      },
    ],
    unbound_plans: [],
    unplanned_requirements: [],
    unplanned_issues: [],
    roles: [],
    resources: [],
    issue_references_by_target: {},
  });

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

test("normalization rejects a mismatched outer and inner object type", () => {
  const invalid = object("goal", "goal", {
    title: "Ship",
    desired_outcome: "A legible project",
    directions: [],
  });
  invalid.data.object_type = "issue";

  assert.throws(
    () => normalizeProjectViewObject(invalid),
    /object type does not match its data/,
  );
});

test("typed Human intents serialize optional relation clears without raw event fields", () => {
  assert.deepEqual(
    serializeProjectViewMutationIntent({
      operation: "initialize",
      profile: {
        name: "Lora",
        positioning: "Shared context",
        purpose: "Coordinate",
        problem: "Fragmentation",
        scope: "Project context",
      },
      goals: [
        {
          title: "Ship",
          desiredOutcome: "One View",
          directions: ["Verify first"],
        },
      ],
    }),
    {
      operation: "initialize",
      profile: {
        name: "Lora",
        positioning: "Shared context",
        purpose: "Coordinate",
        problem: "Fragmentation",
        scope: "Project context",
      },
      goals: [
        {
          title: "Ship",
          desired_outcome: "One View",
          directions: ["Verify first"],
        },
      ],
    },
  );

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
