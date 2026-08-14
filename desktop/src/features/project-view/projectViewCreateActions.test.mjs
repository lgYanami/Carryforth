import assert from "node:assert/strict";
import test from "node:test";

import { projectViewCreateActions } from "./projectViewCreateActions.ts";

function object(objectType, id = objectType) {
  return { id, objectType };
}

test("canonical layers expose only their direct structural create intent", () => {
  assert.deepEqual(
    projectViewCreateActions(object("goal", "goal-1"))
      .filter((action) => action.relation === "structural")
      .map((action) => [action.initialType, action.context]),
    [["plan", { underGoalId: "goal-1" }]],
  );
  assert.deepEqual(
    projectViewCreateActions(object("plan", "plan-1"))
      .filter((action) => action.relation === "structural")
      .map((action) => [action.initialType, action.context]),
    [["stage", { underPlanId: "plan-1" }]],
  );
  assert.deepEqual(
    projectViewCreateActions(object("requirement", "requirement-1"))
      .filter((action) => action.relation === "structural")
      .map((action) => action.context),
    [
      {
        handles: {
          objectId: "requirement-1",
          objectType: "requirement",
        },
      },
    ],
  );
});

test("Stage planned Issue and related Issue remain explicit separate relations", () => {
  const actions = projectViewCreateActions(object("stage", "stage-1"));
  const planned = actions.find((action) => action.id === "issue");
  const related = actions.find((action) => action.id === "related-issue");
  assert.deepEqual(planned?.context, { plannedInStageId: "stage-1" });
  assert.deepEqual(related?.context, {
    about: { objectId: "stage-1", objectType: "stage" },
  });
  assert.equal(
    Object.hasOwn(related?.context ?? {}, "plannedInStageId"),
    false,
  );
});

test("Work, Role, and Resource have no structural child create intent", () => {
  for (const type of ["work", "role", "resource"]) {
    const actions = projectViewCreateActions(object(type));
    assert.deepEqual(
      actions.map((action) => [action.relation, action.initialType]),
      [["related", "issue"]],
    );
  }
});
