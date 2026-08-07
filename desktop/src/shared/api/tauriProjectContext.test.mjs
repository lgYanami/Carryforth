import assert from "node:assert/strict";
import test from "node:test";

import {
  canonicalizeProjectContextQuery,
  ProjectContextError,
  projectContextCoordinateKey,
  projectContextErrorFromPayload,
  projectContextQueryKey,
} from "./tauriProjectContext.ts";

const PROFILE = "00000000-0000-4000-8000-000000000001";
const REQUIREMENT = "00000000-0000-4000-8000-000000000002";
const DOCUMENT = "00000000-0000-4000-8000-000000000003";

const profile = {
  type: "project_view_object",
  objectType: "project_profile",
  objectId: PROFILE,
};
const requirement = {
  type: "project_view_object",
  objectType: "requirement",
  objectId: REQUIREMENT,
};
const document = { type: "document", documentId: DOCUMENT };

test("canonicalizes exact coordinates with the domain family and type order", () => {
  const original = [document, requirement, profile];
  const canonical = canonicalizeProjectContextQuery({
    type: "exact",
    coordinates: original,
  });

  assert.deepEqual(canonical, {
    type: "exact",
    coordinates: [profile, requirement, document],
  });
  assert.deepEqual(original, [document, requirement, profile]);
});

test("contains-all accepts the empty catalog query and one coordinate", () => {
  assert.deepEqual(
    canonicalizeProjectContextQuery({ type: "contains_all", coordinates: [] }),
    { type: "contains_all", coordinates: [] },
  );
  assert.deepEqual(
    canonicalizeProjectContextQuery({
      type: "contains_all",
      coordinates: [requirement],
    }),
    { type: "contains_all", coordinates: [requirement] },
  );
});

test("exact rejects fewer than two coordinates", () => {
  assert.throws(
    () =>
      canonicalizeProjectContextQuery({
        type: "exact",
        coordinates: [requirement],
      }),
    (error) =>
      error instanceof ProjectContextError && error.code === "invalid_input",
  );
});

test("canonicalization rejects duplicate coordinates instead of collapsing them", () => {
  assert.throws(
    () =>
      canonicalizeProjectContextQuery({
        type: "contains_all",
        coordinates: [requirement, { ...requirement }],
      }),
    (error) =>
      error instanceof ProjectContextError && error.code === "invalid_input",
  );
});

test("canonicalization rejects non-v4 identities before invoking native code", () => {
  assert.throws(
    () =>
      canonicalizeProjectContextQuery({
        type: "incident",
        coordinate: { ...document, documentId: "not-a-uuid" },
      }),
    (error) =>
      error instanceof ProjectContextError && error.code === "invalid_input",
  );
});

test("coordinate and query keys are stable across input order", () => {
  assert.equal(
    projectContextCoordinateKey(requirement),
    `requirement:${REQUIREMENT}`,
  );
  assert.equal(
    projectContextQueryKey({
      type: "exact",
      coordinates: [document, requirement],
    }),
    projectContextQueryKey({
      type: "exact",
      coordinates: [requirement, document],
    }),
  );
});

test("maps only closed structured native errors", () => {
  const error = projectContextErrorFromPayload({
    code: "verification_failed",
    message: "Context verification failed.",
    retryable: false,
  });
  assert.ok(error instanceof ProjectContextError);
  assert.equal(error.code, "verification_failed");
  assert.equal(error.retryable, false);

  assert.equal(
    projectContextErrorFromPayload({
      code: "future_error",
      message: "Unknown",
      retryable: false,
    }),
    undefined,
  );
  assert.equal(projectContextErrorFromPayload(new Error("network")), undefined);
});
