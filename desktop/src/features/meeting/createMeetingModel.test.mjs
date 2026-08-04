import assert from "node:assert/strict";
import test from "node:test";

import {
  MAX_MEETING_BOARD_BYTES,
  buildInitialMeetingBoard,
  checkMeetingSourceAccess,
  dedupeMeetingRosterCandidates,
  validateMeetingDraft,
} from "./createMeetingModel.ts";

const HOST = "1".repeat(64);
const HUMAN = "2".repeat(64);
const AGENT = "3".repeat(64);
const INCOMPATIBLE = "4".repeat(64);

test("builds a deterministic complete Markdown Board", () => {
  const input = {
    title: "Lifecycle review",
    goal: "Agree on the delivery boundary.",
    agenda: ["Read model", "", "Create path"],
    background: "V1 is already accepted.",
    references: "https://example.com/spec\nproject:view",
  };
  const first = buildInitialMeetingBoard(input);
  const second = buildInitialMeetingBoard(input);
  assert.equal(first, second);
  assert.equal(
    first,
    "# Lifecycle review\n\n## Discussion goal\n\nAgree on the delivery boundary.\n\n## Agenda\n\n1. Read model\n2. Create path\n\n## Background and context\n\nV1 is already accepted.\n\n## References\n\n- https://example.com/spec\n- project:view\n",
  );
});

test("validates roster bounds, canonical identities and UTF-8 Board bytes", () => {
  assert.deepEqual(
    validateMeetingDraft({
      title: "Review",
      goal: "Decide",
      participantPubkeys: [HUMAN],
      board: "# Review",
    }),
    [],
  );
  const errors = validateMeetingDraft({
    title: "",
    goal: "",
    participantPubkeys: [HUMAN, HUMAN],
    board: `# Goal\n${"界".repeat(MAX_MEETING_BOARD_BYTES)}`,
  });
  assert.ok(errors.some((error) => error.includes("name is required")));
  assert.ok(errors.some((error) => error.includes("goal is required")));
  assert.ok(errors.some((error) => error.includes("duplicate")));
  assert.ok(errors.some((error) => error.includes("byte limit")));
});

test("deduplicates roster sources and classifies Agent capability tri-state", () => {
  const candidates = dedupeMeetingRosterCandidates(
    [
      {
        pubkey: HUMAN,
        displayName: "Human",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: null,
        isAgent: false,
      },
      {
        pubkey: AGENT,
        displayName: "Profile Agent",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: HOST,
        isAgent: true,
      },
      {
        pubkey: AGENT.toUpperCase(),
        displayName: "Relay Agent",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: null,
        isAgent: true,
      },
      {
        pubkey: INCOMPATIBLE,
        displayName: "Old Agent",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: null,
        isAgent: true,
      },
      {
        pubkey: "5".repeat(64),
        displayName: "Unknown Agent",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: null,
        isAgent: true,
      },
    ],
    [
      {
        pubkey: AGENT,
        capabilities: ["meeting-v2-action-finalization-v2"],
      },
      { pubkey: INCOMPATIBLE, capabilities: [] },
    ],
  );
  assert.equal(candidates.length, 4);
  assert.equal(
    candidates.find((item) => item.pubkey === HUMAN)?.actionCapability,
    "not_applicable",
  );
  assert.equal(
    candidates.find((item) => item.pubkey === AGENT)?.actionCapability,
    "compatible",
  );
  assert.equal(
    candidates.find((item) => item.pubkey === INCOMPATIBLE)?.actionCapability,
    "incompatible",
  );
  assert.equal(candidates.at(-1)?.actionCapability, "unknown");
});

test("source access allows none/open and checks every private roster identity", () => {
  assert.equal(
    checkMeetingSourceAccess({
      sourceVisibility: null,
      rosterPubkeys: [HOST, HUMAN],
    }).status,
    "ok",
  );
  assert.equal(
    checkMeetingSourceAccess({
      sourceVisibility: "open",
      rosterPubkeys: [HOST, HUMAN],
    }).status,
    "ok",
  );
  assert.equal(
    checkMeetingSourceAccess({
      sourceVisibility: "private",
      rosterPubkeys: [HOST, HUMAN],
      membersLoading: true,
    }).status,
    "loading",
  );
  assert.equal(
    checkMeetingSourceAccess({
      sourceVisibility: "private",
      rosterPubkeys: [HOST, HUMAN],
      membersUnavailable: true,
    }).status,
    "unavailable",
  );
  assert.deepEqual(
    checkMeetingSourceAccess({
      sourceVisibility: "private",
      rosterPubkeys: [HOST, HUMAN, AGENT],
      memberPubkeys: [HOST, HUMAN],
    }),
    { status: "blocked", missingPubkeys: [AGENT] },
  );
});
