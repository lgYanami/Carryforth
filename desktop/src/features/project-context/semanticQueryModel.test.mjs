import assert from "node:assert/strict";
import test from "node:test";

import {
  addSemanticQueryDraftCoordinate,
  createSemanticQueryDraft,
  removeSemanticQueryDraftCoordinate,
  semanticQueryDraftMatchesSubmission,
  submitSemanticQueryDraft,
  tryAddSemanticQueryDraftCoordinate,
  updateSemanticQueryDraftProblem,
  validateSemanticQueryDraft,
} from "./semanticQueryModel.ts";

const coordinate = (index) => ({
  type: "document",
  documentId: `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`,
});

test("draft validation trims problem and supports empty optional roles", () => {
  const draft = updateSemanticQueryDraftProblem(
    createSemanticQueryDraft(),
    "  why does this recur?  ",
  );
  const validation = validateSemanticQueryDraft(draft);
  assert.equal(validation.valid, true);
  assert.equal(validation.submitted.problem, "why does this recur?");
  assert.deepEqual(validation.submitted.initialCoordinates, []);
  assert.deepEqual(validation.submitted.contextCoordinates, []);
});

test("add is idempotent within one role and permits the same Coordinate across roles", () => {
  const base = updateSemanticQueryDraftProblem(
    createSemanticQueryDraft(),
    "why?",
  );
  const initial = addSemanticQueryDraftCoordinate(
    base,
    "initial",
    coordinate(1),
  );
  const duplicate = tryAddSemanticQueryDraftCoordinate(
    initial,
    "initial",
    coordinate(1),
  );
  assert.equal(duplicate.status, "unchanged");
  assert.equal(duplicate.reason, "duplicate");
  assert.equal(duplicate.draft, initial);

  const crossRole = addSemanticQueryDraftCoordinate(
    initial,
    "context",
    coordinate(1),
  );
  assert.deepEqual(crossRole.initialCoordinates, [coordinate(1)]);
  assert.deepEqual(crossRole.contextCoordinates, [coordinate(1)]);
});

test("enforces 16 Initial and 8 Context Coordinates without throwing", () => {
  let draft = updateSemanticQueryDraftProblem(
    createSemanticQueryDraft(),
    "why?",
  );
  for (let index = 1; index <= 16; index += 1) {
    draft = addSemanticQueryDraftCoordinate(
      draft,
      "initial",
      coordinate(index),
    );
  }
  for (let index = 1; index <= 8; index += 1) {
    draft = addSemanticQueryDraftCoordinate(
      draft,
      "context",
      coordinate(index),
    );
  }
  assert.deepEqual(
    tryAddSemanticQueryDraftCoordinate(draft, "initial", coordinate(17)),
    { status: "unchanged", draft, reason: "limit" },
  );
  assert.deepEqual(
    tryAddSemanticQueryDraftCoordinate(draft, "context", coordinate(9)),
    { status: "unchanged", draft, reason: "limit" },
  );
});

test("remove affects exactly one role", () => {
  const key = `document:${coordinate(1).documentId}`;
  let draft = updateSemanticQueryDraftProblem(
    createSemanticQueryDraft(),
    "why?",
  );
  draft = addSemanticQueryDraftCoordinate(draft, "initial", coordinate(1));
  draft = addSemanticQueryDraftCoordinate(draft, "context", coordinate(1));
  const next = removeSemanticQueryDraftCoordinate(draft, "initial", key);
  assert.deepEqual(next.initialCoordinates, []);
  assert.deepEqual(next.contextCoordinates, [coordinate(1)]);
});

test("submission comparison is canonical and invalid drafts never match", () => {
  let draft = updateSemanticQueryDraftProblem(
    createSemanticQueryDraft(),
    "why?",
  );
  draft = addSemanticQueryDraftCoordinate(draft, "initial", coordinate(2));
  draft = addSemanticQueryDraftCoordinate(draft, "initial", coordinate(1));
  const submitted = submitSemanticQueryDraft(draft);
  assert.equal(semanticQueryDraftMatchesSubmission(draft, submitted), true);
  assert.equal(
    semanticQueryDraftMatchesSubmission(
      updateSemanticQueryDraftProblem(draft, "different"),
      submitted,
    ),
    false,
  );
  assert.equal(
    semanticQueryDraftMatchesSubmission(createSemanticQueryDraft(), submitted),
    false,
  );
});
