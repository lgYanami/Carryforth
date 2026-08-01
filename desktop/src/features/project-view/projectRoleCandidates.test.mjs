import assert from "node:assert/strict";
import test from "node:test";

import { npubEncode } from "nostr-tools/nip19";

import {
  buildProjectRoleCandidates,
  filterProjectRoleCandidates,
  normalizeRoleCandidateInput,
} from "./projectRoleCandidates.ts";

const OWNER = "1".repeat(64);
const MANAGED = "2".repeat(64);
const OTHER_RELAY = "3".repeat(64);
const RELAY_AGENT = "4".repeat(64);
const UNKNOWN_AGENT = "5".repeat(64);
const PERSON_A = "6".repeat(64);
const PERSON_B = "7".repeat(64);
const ROLE = "role-context";

function build(overrides = {}) {
  return buildProjectRoleCandidates({
    activeRelayUrl: "ws://localhost:3000",
    assignments: [],
    currentPubkey: OWNER,
    managedAgents: [],
    managedAgentRuntimes: [],
    members: [{ pubkey: OWNER, role: "owner" }],
    now: Date.parse("2026-07-31T08:00:00Z"),
    profiles: {},
    proposals: [],
    relayAgents: [],
    roles: [{ roleId: ROLE, name: "Context steward" }],
    targetRoleId: ROLE,
    ...overrides,
  });
}

test("includes an unassigned managed Agent from the active Community", () => {
  const matchingAgent = {
    pubkey: MANAGED,
    name: "test-1",
    avatarUrl: "https://example.com/test-1.png",
    backend: { type: "local" },
    relayUrl: "ws://127.0.0.1:3000",
    status: "stopped",
  };
  const candidates = build({
    managedAgents: [
      matchingAgent,
      {
        pubkey: OTHER_RELAY,
        name: "Other Community Agent",
        avatarUrl: null,
        backend: { type: "local" },
        relayUrl: "ws://localhost:3001",
        status: "running",
      },
    ],
  });

  const managed = candidates.find((candidate) => candidate.pubkey === MANAGED);
  assert.equal(managed?.displayName, "test-1");
  assert.equal(managed?.identityType, "agent");
  assert.equal(managed?.managedByCurrentUser, true);
  assert.equal(managed?.runtimeStatus, "stopped");
  assert.equal(managed?.source, "managed");
  assert.equal(
    candidates.some((candidate) => candidate.pubkey === OTHER_RELAY),
    false,
  );
  assert.equal(matchingAgent.relayUrl, "ws://127.0.0.1:3000");
});

test("uses the active runtime pair for a newly created global local Agent", () => {
  const candidates = build({
    managedAgents: [
      {
        pubkey: MANAGED,
        name: "test-2",
        avatarUrl: null,
        backend: { type: "local" },
        relayUrl: "",
        status: "stopped",
      },
      {
        pubkey: OTHER_RELAY,
        name: "Unpaired provider Agent",
        avatarUrl: null,
        backend: { type: "provider", id: "remote", config: {} },
        relayUrl: "",
        status: "deployed",
      },
    ],
    managedAgentRuntimes: [
      {
        pubkey: MANAGED,
        relayUrl: "ws://127.0.0.1:3000",
        localSetup: true,
        lifecycle: "ready",
        pid: 42002,
        error: null,
        logPath: "/tmp/test-2.log",
      },
    ],
  });

  const managed = candidates.find((candidate) => candidate.pubkey === MANAGED);
  assert.equal(managed?.displayName, "test-2");
  assert.equal(managed?.runtimeStatus, "running");
  assert.equal(managed?.source, "managed");
  assert.equal(
    candidates.some((candidate) => candidate.pubkey === OTHER_RELAY),
    false,
  );
});

test("keeps a stopped global local Agent selectable without a runtime row", () => {
  const candidates = build({
    managedAgents: [
      {
        pubkey: MANAGED,
        name: "test-2",
        avatarUrl: null,
        backend: { type: "local" },
        relayUrl: "",
        status: "stopped",
      },
    ],
  });

  const managed = candidates.find((candidate) => candidate.pubkey === MANAGED);
  assert.equal(managed?.displayName, "test-2");
  assert.equal(managed?.runtimeStatus, "stopped");
});

test("deduplicates sources by pubkey and keeps managed presentation data", () => {
  const candidates = build({
    assignments: [
      {
        assignmentId: "assignment-1",
        roleId: "role-builder",
        memberPubkey: MANAGED,
      },
    ],
    managedAgents: [
      {
        pubkey: MANAGED.toUpperCase(),
        name: "Local Agent",
        avatarUrl: "https://example.com/local.png",
        backend: { type: "local" },
        relayUrl: "ws://localhost:3000/",
        status: "running",
      },
    ],
    members: [
      { pubkey: OWNER, role: "owner" },
      { pubkey: MANAGED, role: "member" },
    ],
    profiles: {
      [MANAGED]: {
        displayName: "Profile Agent",
        name: null,
        avatarUrl: "https://example.com/profile.png",
        nip05Handle: "agent@example.com",
        ownerPubkey: OWNER,
        isAgent: true,
      },
    },
    relayAgents: [
      {
        pubkey: MANAGED,
        name: "Relay Agent",
        status: "online",
      },
    ],
    roles: [
      { roleId: ROLE, name: "Context steward" },
      { roleId: "role-builder", name: "Builder" },
    ],
  });

  const merged = candidates.filter((candidate) => candidate.pubkey === MANAGED);
  assert.equal(merged.length, 1);
  assert.equal(merged[0].displayName, "Local Agent");
  assert.equal(merged[0].avatarUrl, "https://example.com/local.png");
  assert.equal(merged[0].communityRole, "member");
  assert.equal(merged[0].activeAssignment?.roleName, "Builder");
});

test("includes owner-backed Relay Agents and excludes unverified directory rows", () => {
  const candidates = build({
    profiles: {
      [RELAY_AGENT]: {
        displayName: "Honey",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: OWNER,
        isAgent: true,
      },
      [UNKNOWN_AGENT]: {
        displayName: "Unknown",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: OTHER_RELAY,
        isAgent: true,
      },
    },
    relayAgents: [
      { pubkey: RELAY_AGENT, name: "Honey", status: "away" },
      { pubkey: UNKNOWN_AGENT, name: "Unknown", status: "online" },
    ],
  });

  assert.equal(
    candidates.find((candidate) => candidate.pubkey === RELAY_AGENT)
      ?.runtimeStatus,
    "away",
  );
  assert.equal(
    candidates.some((candidate) => candidate.pubkey === UNKNOWN_AGENT),
    false,
  );
});

test("keeps same-name identities distinct and ranks name and pubkey searches", () => {
  const candidates = build({
    members: [
      { pubkey: OWNER, role: "owner" },
      { pubkey: PERSON_A, role: "member" },
      { pubkey: PERSON_B, role: "member" },
    ],
    profiles: {
      [PERSON_A]: {
        displayName: "Alex",
        avatarUrl: null,
        nip05Handle: "alex-a@example.com",
        ownerPubkey: null,
      },
      [PERSON_B]: {
        displayName: "Alex",
        avatarUrl: null,
        nip05Handle: "alex-b@example.com",
        ownerPubkey: null,
      },
    },
  });

  const alexes = filterProjectRoleCandidates(candidates, "Alex").people;
  assert.deepEqual(
    alexes.map((candidate) => candidate.pubkey),
    [PERSON_A, PERSON_B],
  );
  assert.deepEqual(
    filterProjectRoleCandidates(candidates, PERSON_B.slice(0, 16)).people.map(
      (candidate) => candidate.pubkey,
    ),
    [PERSON_B],
  );
});

test("normalizes manual hex and npub input and rejects malformed identities", () => {
  assert.equal(
    normalizeRoleCandidateInput(` ${MANAGED.toUpperCase()} `),
    MANAGED,
  );
  assert.equal(normalizeRoleCandidateInput(npubEncode(MANAGED)), MANAGED);
  assert.equal(normalizeRoleCandidateInput("npub1not-valid"), null);
  assert.equal(normalizeRoleCandidateInput("abc123"), null);
});
