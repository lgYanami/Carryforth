import assert from "node:assert/strict";
import test from "node:test";

import { validateProjectContextSearch } from "./project-context.tsx";
import {
  projectContextRouteSearchForState,
  projectContextRouteStateFromSearch,
} from "../../features/project-context/routeState.ts";

const REQUIREMENT = "20000000-0000-4000-8000-000000000001";
const RESOURCE = "30000000-0000-4000-8000-000000000001";
const DOCUMENT = "40000000-0000-4000-8000-000000000001";
const EDGE = "ab".repeat(32);

test("omitted query and contains-all empty set share one canonical All route", () => {
  assert.deepEqual(validateProjectContextSearch({}), {});
  assert.deepEqual(
    validateProjectContextSearch({
      mode: "contains_all",
      coordinates: "",
      foreign: "discarded",
    }),
    {},
  );
});

test("exact Coordinate order and UUID case canonicalize in the URL", () => {
  assert.deepEqual(
    validateProjectContextSearch({
      mode: "exact",
      coordinates: `resource:${RESOURCE.toUpperCase()},requirement:${REQUIREMENT}`,
    }),
    {
      mode: "exact",
      coordinates: `requirement:${REQUIREMENT},resource:${RESOURCE}`,
    },
  );
});

test("route parser restores typed Incident query and selection independently", () => {
  const search = validateProjectContextSearch({
    mode: "incident",
    coordinates: `document:${DOCUMENT}`,
    selected: `edge:${EDGE.toUpperCase()}`,
  });
  assert.deepEqual(search, {
    mode: "incident",
    coordinates: `document:${DOCUMENT}`,
    selected: `edge:${EDGE}`,
  });
  assert.deepEqual(projectContextRouteStateFromSearch(search), {
    query: {
      type: "incident",
      coordinate: { type: "document", documentId: DOCUMENT },
    },
    selection: { kind: "edge", key: EDGE },
  });
});

test("selection updates have a unique encoding without changing the query", () => {
  const query = {
    type: "contains_all",
    coordinates: [
      {
        type: "project_view_object",
        objectType: "requirement",
        objectId: REQUIREMENT,
      },
    ],
  };
  assert.deepEqual(
    projectContextRouteSearchForState(query, {
      kind: "coordinate",
      key: `document:${DOCUMENT}`,
    }),
    {
      mode: "contains_all",
      coordinates: `requirement:${REQUIREMENT}`,
      selected: `coordinate:document:${DOCUMENT}`,
    },
  );
  assert.deepEqual(
    projectContextRouteSearchForState(
      { type: "contains_all", coordinates: [] },
      { kind: "edge", key: EDGE },
    ),
    { selected: `edge:${EDGE}` },
  );
});

test("malformed, duplicate, and mode-invalid route state is explicit", () => {
  for (const search of [
    { mode: "all" },
    { coordinates: `document:${DOCUMENT}` },
    { mode: "incident", coordinates: "" },
    {
      mode: "incident",
      coordinates: `document:${DOCUMENT},resource:${RESOURCE}`,
    },
    { mode: "exact", coordinates: `document:${DOCUMENT}` },
    {
      mode: "exact",
      coordinates: `document:${DOCUMENT},document:${DOCUMENT}`,
    },
    { mode: "contains_all", coordinates: [DOCUMENT] },
    { selected: "edge:not-a-hash" },
    { selected: `coordinate:unknown:${DOCUMENT}` },
  ]) {
    const validated = validateProjectContextSearch(search);
    assert.equal(typeof validated.invalid, "string", JSON.stringify(search));
  }
});
