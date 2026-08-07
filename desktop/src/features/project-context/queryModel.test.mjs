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
} from "./queryModel.ts";

const REQUIREMENT_ID = "20000000-0000-4000-8000-000000000001";
const RESOURCE_ID = "30000000-0000-4000-8000-000000000001";
const DOCUMENT_ID = "40000000-0000-4000-8000-000000000001";
const TOMBSTONE_ID = "50000000-0000-4000-8000-000000000001";
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

test("Incident accepts one Coordinate and duplicate input is rejected", () => {
  let draft = changeProjectContextDraftMode(
    projectContextDraftFromQuery({ type: "contains_all", coordinates: [] }),
    "incident",
  );
  draft = addProjectContextDraftCoordinate(draft, document);
  assert.deepEqual(projectContextQueryFromDraft(draft), {
    type: "incident",
    coordinate: document,
  });
  assert.throws(
    () => addProjectContextDraftCoordinate(draft, document),
    /already/,
  );
  assert.throws(
    () => addProjectContextDraftCoordinate(draft, requirement),
    /exactly one/,
  );
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
