import assert from "node:assert/strict";
import test from "node:test";

import {
  isProjectViewIntegrityError,
  normalizeProjectViewLoadResult,
} from "./tauriProjectView.ts";

const ACTOR = "a".repeat(64);
const NOW = "2026-07-28T00:00:00Z";

function object(objectType, id, data, relations = {}) {
  return {
    id,
    object_type: objectType,
    object_revision: 1,
    project_revision: 1,
    created_at: NOW,
    updated_at: NOW,
    created_by: ACTOR,
    updated_by: ACTOR,
    data: { object_type: objectType, data },
    relations,
  };
}

function readyResult() {
  return {
    status: "ready",
    relay_pubkey: "b".repeat(64),
    project_revision: 1,
    projection_generation: 1,
    active_object_count: 2,
    updated_at: NOW,
    view: {
      profile: object(
        "project_profile",
        "00000000-0000-4000-8000-000000000001",
        {
          name: "Project",
          positioning: "Positioning",
          purpose: "Purpose",
          problem: "Problem",
          scope: "Scope",
        },
      ),
      goals: [
        {
          goal: object("goal", "00000000-0000-4000-8000-000000000002", {
            title: "Goal",
            desired_outcome: "Outcome",
            directions: [],
          }),
          plans: [],
        },
      ],
      unbound_plans: [],
      unplanned_requirements: [],
      unplanned_issues: [],
      roles: [],
      resources: [],
      issue_references_by_target: {},
    },
  };
}

test("accepts one internally consistent native Project View DTO", () => {
  const result = normalizeProjectViewLoadResult(readyResult());

  assert.equal(result.status, "ready");
  assert.equal(result.activeObjectCount, 2);
  assert.equal(result.view.goals[0].goal.data.title, "Goal");
});

test("rejects an active object count that cannot describe the assembled View", () => {
  const raw = readyResult();
  raw.active_object_count = 3;

  assert.throws(
    () => normalizeProjectViewLoadResult(raw),
    /active object count 3 does not match the 2 assembled objects/,
  );
});

test("rejects an object from a future project revision", () => {
  const raw = readyResult();
  raw.view.goals[0].goal.project_revision = 2;

  assert.throws(
    () => normalizeProjectViewLoadResult(raw),
    /belongs to an impossible project revision/,
  );
});

test("rejects a relation tree that disagrees with the object relation", () => {
  const raw = readyResult();
  const plan = object(
    "plan",
    "00000000-0000-4000-8000-000000000003",
    {
      title: "Plan",
      description: "Description",
      status: "active",
    },
    { under_goal_id: "00000000-0000-4000-8000-000000000099" },
  );
  raw.view.goals[0].plans.push({ plan, stages: [] });
  raw.active_object_count = 3;

  assert.throws(
    () => normalizeProjectViewLoadResult(raw),
    /is under the wrong Goal/,
  );
});

test("classifies native and client integrity failures without class identity", () => {
  assert.equal(
    isProjectViewIntegrityError(
      new Error("Project View integrity error: invalid relay projection"),
    ),
    true,
  );
  assert.equal(
    isProjectViewIntegrityError(new Error("Relay unavailable")),
    false,
  );
});
