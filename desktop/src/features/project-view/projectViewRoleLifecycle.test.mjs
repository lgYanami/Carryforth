import assert from "node:assert/strict";
import test from "node:test";

import { projectViewRoleLifecycleState } from "./projectViewRoleLifecycle.ts";

const definition = { roleId: "role-1" };

function continuity(overrides = {}) {
  return {
    assignments: [],
    proposals: [],
    workResponsibilities: [],
    ...overrides,
  };
}

test("Role lifecycle fence includes Assignment, Proposal, and responsible Work", () => {
  const assignment = projectViewRoleLifecycleState(
    definition,
    continuity({ assignments: [{ roleId: "role-1" }] }),
  );
  assert.equal(assignment.blocked, true);
  assert.equal(assignment.hasActiveAssignment, true);
  assert.match(assignment.message, /active Assignment/);

  const proposal = projectViewRoleLifecycleState(
    definition,
    continuity({ proposals: [{ roleId: "role-1", status: "open" }] }),
  );
  assert.equal(proposal.hasOpenProposal, true);
  assert.match(proposal.message, /open Proposal/);

  const work = projectViewRoleLifecycleState(
    definition,
    continuity({ workResponsibilities: [{ roleId: "role-1" }] }),
  );
  assert.equal(work.hasResponsibleWork, true);
  assert.match(work.message, /responsible for Work/);
});

test("ended Assignments and resolved Proposals do not block lifecycle", () => {
  const state = projectViewRoleLifecycleState(
    definition,
    continuity({
      assignments: [{ roleId: "role-1", endedAt: "2026-08-15" }],
      proposals: [{ roleId: "role-1", status: "accepted" }],
    }),
  );
  assert.deepEqual(state, {
    blocked: false,
    hasActiveAssignment: false,
    hasOpenProposal: false,
    hasResponsibleWork: false,
    message: undefined,
  });
});
