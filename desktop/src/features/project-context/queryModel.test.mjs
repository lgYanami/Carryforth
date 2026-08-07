import assert from "node:assert/strict";
import test from "node:test";

import {
  addProjectContextDraftCoordinate,
  buildProjectContextCoordinateOptions,
  changeProjectContextDraftMode,
  projectContextDraftFromQuery,
  projectContextDraftValidationMessage,
  projectContextQueryFromDraft,
  removeProjectContextDraftCoordinate,
  tryAddProjectContextDraftCoordinate,
} from "./queryModel.ts";

const REQUIREMENT_ID = "20000000-0000-4000-8000-000000000001";
const RESOURCE_ID = "30000000-0000-4000-8000-000000000001";
const DOCUMENT_ID = "40000000-0000-4000-8000-000000000001";
const TOMBSTONE_ID = "50000000-0000-4000-8000-000000000001";
const MEETING_ID = "60000000-0000-4000-8000-000000000001";
const requirement = {
  type: "project_view_object",
  objectType: "requirement",
  objectId: REQUIREMENT_ID,
};
const resource = {
  type: "project_view_object",
  objectType: "resource",
  objectId: RESOURCE_ID,
};
const document = { type: "document", documentId: DOCUMENT_ID };

test("draft mode constraints stay distinct from the applied query", () => {
  const applied = { type: "contains_all", coordinates: [] };
  let draft = projectContextDraftFromQuery(applied);
  assert.deepEqual(draft, { mode: "all", coordinates: [] });

  draft = changeProjectContextDraftMode(draft, "exact");
  assert.match(projectContextDraftValidationMessage(draft), /at least two/);
  draft = addProjectContextDraftCoordinate(draft, resource);
  draft = addProjectContextDraftCoordinate(draft, requirement);
  assert.equal(projectContextDraftValidationMessage(draft), undefined);
  assert.deepEqual(projectContextQueryFromDraft(draft), {
    type: "exact",
    coordinates: [requirement, resource],
  });
  assert.deepEqual(applied, { type: "contains_all", coordinates: [] });
});

test("Incident accepts one Coordinate and rejects repeated input without throwing", () => {
  let draft = changeProjectContextDraftMode(
    projectContextDraftFromQuery({ type: "contains_all", coordinates: [] }),
    "incident",
  );
  draft = addProjectContextDraftCoordinate(draft, document);
  assert.deepEqual(projectContextQueryFromDraft(draft), {
    type: "incident",
    coordinate: document,
  });
  const duplicate = tryAddProjectContextDraftCoordinate(draft, document);
  assert.deepEqual(duplicate, {
    status: "unchanged",
    draft,
    reason: "duplicate",
  });
  const incidentFull = tryAddProjectContextDraftCoordinate(draft, requirement);
  assert.deepEqual(incidentFull, {
    status: "unchanged",
    draft,
    reason: "incident_full",
  });
  assert.strictEqual(
    addProjectContextDraftCoordinate(draft, requirement),
    draft,
  );
});

test("All rejects stale Coordinate selection as an idempotent transition", () => {
  const draft = projectContextDraftFromQuery({
    type: "contains_all",
    coordinates: [],
  });
  assert.deepEqual(tryAddProjectContextDraftCoordinate(draft, requirement), {
    status: "unchanged",
    draft,
    reason: "mode_all",
  });
});

test("switching a multi-Coordinate draft to Incident keeps one canonical Coordinate", () => {
  let draft = changeProjectContextDraftMode(
    projectContextDraftFromQuery({ type: "contains_all", coordinates: [] }),
    "exact",
  );
  draft = addProjectContextDraftCoordinate(draft, resource);
  draft = addProjectContextDraftCoordinate(draft, requirement);
  assert.deepEqual(changeProjectContextDraftMode(draft, "incident"), {
    mode: "incident",
    coordinates: [requirement],
  });
});

test("Contains all supports one Coordinate while empty is represented by All", () => {
  let draft = changeProjectContextDraftMode(
    projectContextDraftFromQuery({ type: "contains_all", coordinates: [] }),
    "contains_all",
  );
  assert.match(projectContextDraftValidationMessage(draft), /at least one/);
  draft = addProjectContextDraftCoordinate(draft, requirement);
  assert.deepEqual(projectContextQueryFromDraft(draft), {
    type: "contains_all",
    coordinates: [requirement],
  });
  draft = removeProjectContextDraftCoordinate(
    draft,
    `requirement:${REQUIREMENT_ID}`,
  );
  assert.match(projectContextDraftValidationMessage(draft), /All Context/);
  assert.deepEqual(changeProjectContextDraftMode(draft, "all"), {
    mode: "all",
    coordinates: [],
  });
});

test("picker groups active catalogs and retains visible lifecycle Coordinates", () => {
  const options = buildProjectContextCoordinateOptions({
    projectViewObjects: [
      {
        id: REQUIREMENT_ID,
        objectType: "requirement",
        data: { title: "Verified requirement", status: "accepted" },
      },
    ],
    documents: [
      {
        documentId: DOCUMENT_ID,
        title: "Architecture notes",
        summary: "Shared rationale",
      },
    ],
    visibleDetails: [
      {
        coordinateKey: `requirement:${REQUIREMENT_ID}`,
        coordinate: requirement,
        state: "unavailable",
        title: "Stale duplicate",
      },
      {
        coordinateKey: `resource:${TOMBSTONE_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "resource",
          objectId: TOMBSTONE_ID,
        },
        state: "tombstoned",
        title: "Retired resource",
      },
    ],
  });

  assert.deepEqual(
    options.map((option) => [
      option.coordinateKey,
      option.group,
      option.state,
      option.title,
    ]),
    [
      [
        `requirement:${REQUIREMENT_ID}`,
        "project_view",
        "active",
        "Verified requirement",
      ],
      [
        `resource:${TOMBSTONE_ID}`,
        "project_view",
        "tombstoned",
        "Retired resource",
      ],
      [`document:${DOCUMENT_ID}`, "documents", "active", "Architecture notes"],
    ],
  );
  assert.equal(options[0].status, "Accepted");
  assert.equal(options[2].description, "Shared rationale");
});

test("picker exposes only terminal Meetings with searchable participant presentation", () => {
  const options = buildProjectContextCoordinateOptions({
    meetings: [
      {
        meetingId: MEETING_ID,
        title: "Memory boundary review",
        description: "Agree the first durable memory slice",
        lifecycle: "closed",
        phase: "ended",
        currentSpeakerPubkey: null,
        currentOfferPubkey: null,
        needsAttention: false,
        attentionReason: null,
        moderatorPubkey: "a".repeat(64),
        hostPubkey: "a".repeat(64),
        participantCount: 4,
        participantPreview: [
          {
            pubkey: "a".repeat(64),
            participantType: "human",
            channelRole: "admin",
          },
          {
            pubkey: "b".repeat(64),
            participantType: "agent",
            channelRole: "member",
          },
        ],
        viewerRole: "observer",
        policy: "moderated-board-actions-v3",
        createdAt: 1_786_054_800,
        updatedAt: 1_786_055_400,
        endedAt: 1_786_055_400,
        latestSpeechAt: 1_786_055_300,
        compatibility: "ready",
      },
      {
        meetingId: "70000000-0000-4000-8000-000000000001",
        title: "Still running",
        description: null,
        lifecycle: "active",
        phase: "floor_ready",
        currentSpeakerPubkey: null,
        currentOfferPubkey: null,
        needsAttention: false,
        attentionReason: null,
        moderatorPubkey: "a".repeat(64),
        hostPubkey: "a".repeat(64),
        participantCount: 2,
        participantPreview: [],
        viewerRole: "participant",
        policy: "moderated-board-actions-v3",
        createdAt: 1_786_055_500,
        updatedAt: 1_786_055_500,
        endedAt: null,
        latestSpeechAt: null,
        compatibility: "ready",
      },
    ],
    profiles: {
      ["a".repeat(64)]: { pubkey: "a".repeat(64), displayName: "Ada" },
      ["b".repeat(64)]: { pubkey: "b".repeat(64), displayName: "Bumble" },
    },
  });

  assert.equal(options.length, 1);
  assert.deepEqual(options[0].coordinate, {
    type: "meeting",
    meetingId: MEETING_ID,
  });
  assert.equal(options[0].coordinateKey, `meeting:${MEETING_ID}`);
  assert.equal(options[0].group, "meetings");
  assert.equal(options[0].state, "terminal");
  assert.match(options[0].status, /^Closed/);
  assert.match(options[0].searchTerms, /Ada/);
  assert.match(options[0].searchTerms, /Bumble/);
});
