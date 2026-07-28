import assert from "node:assert/strict";
import test from "node:test";

import { countProjectViewFocus, indexProjectViewObjects } from "./model.ts";
import {
  normalizeProjectView,
  normalizeProjectViewObject,
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
