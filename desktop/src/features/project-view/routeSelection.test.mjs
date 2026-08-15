import assert from "node:assert/strict";
import test from "node:test";

import {
  projectViewRouteForSelection,
  projectViewSelectionFromRoute,
  validateProjectViewRouteSearch,
} from "./routeSelection.ts";

test("object identity wins and clears an ambiguous Document coordinate", () => {
  assert.deepEqual(
    validateProjectViewRouteSearch({
      object: "plan-1",
      document: "document-1",
      revision: "4",
      via: "document-context:plan-1:document-1:pinned:4",
    }),
    { object: "plan-1", via: "document-context:plan-1:document-1:pinned:4" },
  );
});

test("object routes discard orphan revisions and empty values", () => {
  assert.deepEqual(
    validateProjectViewRouteSearch({
      object: "plan-1",
      revision: 9,
      via: "",
    }),
    { object: "plan-1", via: undefined },
  );
  assert.deepEqual(
    validateProjectViewRouteSearch({ revision: 9, via: "orphan" }),
    {},
  );
});

test("invalid revisions fail closed without removing a Document identity", () => {
  assert.deepEqual(
    validateProjectViewRouteSearch({ document: "document-1", revision: 0 }),
    { document: "document-1", revision: undefined, via: undefined },
  );
});

test("selection serialization replaces the entire identity union", () => {
  const documentSelection = {
    kind: "document",
    documentId: "document-1",
    revision: 3,
    via: "document-context:goal-1:document-1:pinned:3",
  };
  const route = projectViewRouteForSelection(documentSelection);
  assert.deepEqual(route, {
    document: "document-1",
    revision: 3,
    via: "document-context:goal-1:document-1:pinned:3",
  });
  assert.deepEqual(projectViewSelectionFromRoute(route), documentSelection);
  assert.deepEqual(projectViewRouteForSelection(undefined), {});
});
