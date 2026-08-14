import assert from "node:assert/strict";
import test from "node:test";

import {
  expandProjectViewOutlineAncestors,
  indexProjectViewOutline,
  navigateProjectViewOutline,
  reconcileProjectViewOutlineExpanded,
  visibleProjectViewCurrentContainer,
  visibleProjectViewOutlineNodes,
} from "./outlineState.ts";

function object(id, title, children = []) {
  return {
    kind: "object",
    occurrenceKey: `object:${id}:canonical`,
    relation: id === "profile" ? "root" : "structural",
    object: {
      id,
      objectType: id === "profile" ? "project_profile" : "goal",
      data: id === "profile" ? { name: title } : { title },
    },
    title,
    children,
  };
}

function group(key, label, children) {
  return { kind: "group", occurrenceKey: key, label, children };
}

function fixture() {
  const plan = object("plan", "Plan");
  const plans = group("group:goal:plans", "Plans", [plan]);
  const goal = object("goal", "Goal", [plans]);
  const goals = group("group:profile:goals", "Goals", [goal]);
  const role = object("role", "Role");
  const roles = group("group:profile:roles", "Roles", [role]);
  const root = object("profile", "Project", [goals, roles]);
  return { root, goals, goal, plans, plan, roles, role };
}

test("ancestor expansion preserves valid user branches and drops stale keys", () => {
  const nodes = fixture();
  const index = indexProjectViewOutline(nodes.root);
  const expanded = expandProjectViewOutlineAncestors(
    index,
    nodes.plan.occurrenceKey,
    new Set([nodes.roles.occurrenceKey, "missing", nodes.role.occurrenceKey]),
  );
  assert.deepEqual(
    [...expanded].sort(),
    [
      nodes.goal.occurrenceKey,
      nodes.goals.occurrenceKey,
      nodes.plans.occurrenceKey,
      nodes.roles.occurrenceKey,
      nodes.root.occurrenceKey,
    ].sort(),
  );
});

test("visible projection mounts only expanded branches", () => {
  const nodes = fixture();
  const index = indexProjectViewOutline(nodes.root);
  assert.deepEqual(
    visibleProjectViewOutlineNodes(index, new Set()).map(
      (node) => node.occurrenceKey,
    ),
    [nodes.root.occurrenceKey],
  );
  assert.deepEqual(
    visibleProjectViewOutlineNodes(
      index,
      new Set([nodes.root.occurrenceKey, nodes.goals.occurrenceKey]),
    ).map((node) => node.occurrenceKey),
    [
      nodes.root.occurrenceKey,
      nodes.goals.occurrenceKey,
      nodes.goal.occurrenceKey,
      nodes.roles.occurrenceKey,
    ],
  );
});

test("collapsed current paths report the nearest visible ancestor", () => {
  const nodes = fixture();
  const index = indexProjectViewOutline(nodes.root);
  const expanded = new Set([
    nodes.root.occurrenceKey,
    nodes.goals.occurrenceKey,
    nodes.goal.occurrenceKey,
  ]);
  assert.equal(
    visibleProjectViewCurrentContainer(
      index,
      nodes.plan.occurrenceKey,
      expanded,
    ),
    nodes.plans.occurrenceKey,
  );
  assert.equal(
    visibleProjectViewCurrentContainer(
      index,
      nodes.goal.occurrenceKey,
      expanded,
    ),
    nodes.goal.occurrenceKey,
  );
});

test("tree keyboard navigation expands, enters, collapses, and never wraps", () => {
  const nodes = fixture();
  const index = indexProjectViewOutline(nodes.root);
  let state = navigateProjectViewOutline({
    index,
    expandedKeys: new Set(),
    focusedKey: nodes.root.occurrenceKey,
    key: "ArrowRight",
  });
  assert.equal(state.focusedKey, nodes.root.occurrenceKey);
  assert.equal(state.expandedKeys.has(nodes.root.occurrenceKey), true);

  state = navigateProjectViewOutline({
    index,
    expandedKeys: state.expandedKeys,
    focusedKey: state.focusedKey,
    key: "ArrowRight",
  });
  assert.equal(state.focusedKey, nodes.goals.occurrenceKey);

  state = navigateProjectViewOutline({
    index,
    expandedKeys: new Set([nodes.root.occurrenceKey]),
    focusedKey: nodes.roles.occurrenceKey,
    key: "ArrowDown",
  });
  assert.equal(state.focusedKey, nodes.roles.occurrenceKey);

  state = navigateProjectViewOutline({
    index,
    expandedKeys: new Set([nodes.root.occurrenceKey]),
    focusedKey: nodes.goals.occurrenceKey,
    key: "ArrowLeft",
  });
  assert.equal(state.focusedKey, nodes.root.occurrenceKey);

  state = navigateProjectViewOutline({
    index,
    expandedKeys: new Set([nodes.root.occurrenceKey]),
    focusedKey: nodes.goals.occurrenceKey,
    key: "End",
  });
  assert.equal(state.focusedKey, nodes.roles.occurrenceKey);
  state = navigateProjectViewOutline({
    index,
    expandedKeys: state.expandedKeys,
    focusedKey: state.focusedKey,
    key: "Home",
  });
  assert.equal(state.focusedKey, nodes.root.occurrenceKey);
});

test("expanded reconciliation retains only expandable live nodes", () => {
  const nodes = fixture();
  const index = indexProjectViewOutline(nodes.root);
  assert.deepEqual(
    [
      ...reconcileProjectViewOutlineExpanded(
        index,
        new Set([
          nodes.root.occurrenceKey,
          nodes.role.occurrenceKey,
          "missing",
        ]),
      ),
    ],
    [nodes.root.occurrenceKey],
  );
});
