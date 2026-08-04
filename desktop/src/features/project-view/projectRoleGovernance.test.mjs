import assert from "node:assert/strict";
import test from "node:test";

import {
  canGovernProjectRole,
  projectRoleGovernanceCapabilities,
} from "./projectRoleGovernance.ts";

function continuity(membership, roleLevel) {
  return {
    roles: roleLevel
      ? [
          {
            roleId: "role-1",
            name: "Leader",
            purpose: "Govern",
            responsibilities: [],
            boundaries: [],
            level: roleLevel,
            active: true,
            objectRevision: 1,
            projectRevision: 1,
            createdAt: "2026-08-03T00:00:00Z",
            updatedAt: "2026-08-03T00:00:00Z",
            createdBy: "actor",
            updatedBy: "actor",
          },
        ]
      : [],
    assignments: roleLevel
      ? [
          {
            assignmentId: "assignment-1",
            roleId: "role-1",
            memberPubkey: "actor",
            startedAt: "2026-08-03T00:00:00Z",
            startedBy: "owner",
            entityRevision: 1,
            projectRevision: 1,
          },
        ]
      : [],
    proposals: [],
    commitments: [],
    workResponsibilities: [],
    checkpoints: [],
    handoffs: [],
    members: [{ pubkey: "actor", role: membership }],
    briefs: [],
  };
}

test("owner governs both Role levels without an Assignment", () => {
  const capabilities = projectRoleGovernanceCapabilities(
    continuity("owner"),
    "actor",
  );

  assert.equal(capabilities.canCreateAdminRole, true);
  assert.equal(capabilities.actingAssignmentId, undefined);
  assert.equal(canGovernProjectRole(capabilities, "admin"), true);
  assert.equal(canGovernProjectRole(capabilities, "member"), true);
});

test("Leader requires both admin membership and active admin Assignment", () => {
  const leader = projectRoleGovernanceCapabilities(
    continuity("admin", "admin"),
    "actor",
  );
  const membershipOnly = projectRoleGovernanceCapabilities(
    continuity("admin"),
    "actor",
  );
  const assignmentOnly = projectRoleGovernanceCapabilities(
    continuity("member", "admin"),
    "actor",
  );

  assert.equal(leader.actingAssignmentId, "assignment-1");
  assert.equal(canGovernProjectRole(leader, "member"), true);
  assert.equal(canGovernProjectRole(leader, "admin"), false);
  assert.equal(membershipOnly.canCreateMemberRole, false);
  assert.equal(assignmentOnly.canCreateMemberRole, false);
});
