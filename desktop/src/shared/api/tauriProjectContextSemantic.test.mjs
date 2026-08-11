import assert from "node:assert/strict";
import test from "node:test";

import {
  canonicalizeSemanticProjectContextProblem,
  canonicalizeSemanticProjectContextQueryInput,
  requireMatchingSemanticProjectContextResponse,
  SemanticProjectContextError,
  semanticProjectContextErrorFromPayload,
  semanticProjectContextProblemBytes,
} from "./tauriProjectContextSemantic.ts";

const IDENTITY = {
  communityKey: "community-a-0",
  appliedWorkspaceToken: "workspace-token-a",
  callerPubkey: "a".repeat(64),
  projectId: "00000000-0000-4000-8000-000000000001",
  relayPubkey: "b".repeat(64),
};

const DOCUMENT = {
  type: "document",
  documentId: "00000000-0000-4000-8000-000000000002",
};

const RESULT = {
  ...IDENTITY,
  requestId: "00000000-0000-4000-8000-000000000003",
  projectContextRevision: 42,
  snapshotObservedAt: "2026-08-11T00:00:00Z",
  completionReason: "frontier_exhausted",
  exhaustedDimensions: [],
  coverage: {
    authorizedGraphSources: 1,
    currentIndexedGraphSources: 1,
    titleOnlySources: 0,
    rootsReturned: 0,
    pathsReturned: 0,
    omittedInitialCoordinates: 0,
    omittedContextCoordinates: 0,
    indexCoveragePartial: 0,
    omittedForResponseBudget: { automaticRoots: 0, paths: 0, summaries: 0 },
  },
  inputOutcomes: { initial: [], context: [] },
  roots: [],
  paths: [],
};

test("validates the problem by trimmed UTF-8 bytes", () => {
  assert.equal(semanticProjectContextProblemBytes(" 中 "), 3);
  assert.equal(
    canonicalizeSemanticProjectContextProblem("  explain this  "),
    "explain this",
  );
  assert.equal(
    canonicalizeSemanticProjectContextProblem("中".repeat(5_461)),
    "中".repeat(5_461),
  );
  assert.throws(
    () => canonicalizeSemanticProjectContextProblem("中".repeat(5_462)),
    (error) =>
      error instanceof SemanticProjectContextError &&
      error.code === "invalid_input",
  );
});

test("rejects blank and NUL problems", () => {
  for (const problem of [" \n\t ", "why\0now"]) {
    assert.throws(
      () => canonicalizeSemanticProjectContextProblem(problem),
      (error) =>
        error instanceof SemanticProjectContextError &&
        error.code === "invalid_input",
    );
  }
});

test("constructs a closed native payload and never forwards lifecycle or budget", () => {
  const canonical = canonicalizeSemanticProjectContextQueryInput({
    communityKey: IDENTITY.communityKey,
    appliedWorkspaceToken: IDENTITY.appliedWorkspaceToken,
    problem: "  why?  ",
    initialCoordinates: [DOCUMENT],
    contextCoordinates: [DOCUMENT],
    lifecycleFilter: "terminal_only",
    budget: { paths: 999 },
  });

  assert.deepEqual(canonical, {
    communityKey: IDENTITY.communityKey,
    appliedWorkspaceToken: IDENTITY.appliedWorkspaceToken,
    problem: "why?",
    initialCoordinates: [DOCUMENT],
    contextCoordinates: [DOCUMENT],
  });
});

test("allows a Coordinate across roles but rejects duplicates within one role", () => {
  assert.doesNotThrow(() =>
    canonicalizeSemanticProjectContextQueryInput({
      communityKey: IDENTITY.communityKey,
      appliedWorkspaceToken: IDENTITY.appliedWorkspaceToken,
      problem: "why?",
      initialCoordinates: [DOCUMENT],
      contextCoordinates: [DOCUMENT],
    }),
  );
  assert.throws(
    () =>
      canonicalizeSemanticProjectContextQueryInput({
        communityKey: IDENTITY.communityKey,
        appliedWorkspaceToken: IDENTITY.appliedWorkspaceToken,
        problem: "why?",
        initialCoordinates: [DOCUMENT, { ...DOCUMENT }],
        contextCoordinates: [],
      }),
    (error) =>
      error instanceof SemanticProjectContextError &&
      error.code === "invalid_input",
  );
});

test("accepts a result only for the exact workspace, caller, Project, and Relay", () => {
  assert.equal(
    requireMatchingSemanticProjectContextResponse(IDENTITY, RESULT),
    RESULT,
  );

  assert.throws(
    () =>
      requireMatchingSemanticProjectContextResponse(IDENTITY, {
        ...RESULT,
        appliedWorkspaceToken: "workspace-token-b",
      }),
    (error) =>
      error instanceof SemanticProjectContextError &&
      error.code === "verification_failed",
  );
  for (const mismatch of [
    { callerPubkey: "c".repeat(64) },
    { projectId: "00000000-0000-4000-8000-000000000004" },
    { relayPubkey: "d".repeat(64) },
  ]) {
    assert.throws(
      () =>
        requireMatchingSemanticProjectContextResponse(IDENTITY, {
          ...RESULT,
          ...mismatch,
        }),
      (error) =>
        error instanceof SemanticProjectContextError &&
        error.code === "verification_failed",
    );
  }
});

test("maps only the closed native semantic error vocabulary", () => {
  const error = semanticProjectContextErrorFromPayload({
    code: "busy",
    message: "Provider is busy.",
    retryable: true,
    retryAfterSeconds: 2,
  });
  assert.ok(error instanceof SemanticProjectContextError);
  assert.equal(error.code, "busy");
  assert.equal(error.retryAfterSeconds, 2);
  assert.equal(
    semanticProjectContextErrorFromPayload({
      code: "future_code",
      message: "unknown",
      retryable: false,
    }),
    undefined,
  );
});
