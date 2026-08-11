import assert from "node:assert/strict";
import test from "node:test";

import { SemanticProjectContextError } from "../../shared/api/tauriProjectContextSemantic.ts";
import {
  createSemanticUiState,
  nextSemanticAttemptToken,
  semanticOverlayEligible,
  semanticQueryRequiresAllContext,
  semanticSessionFreshness,
  semanticSessionReducer,
} from "./semanticSession.ts";

const IDENTITY = {
  communityKey: "community-a-0",
  appliedWorkspaceToken: "workspace-token-a",
  callerPubkey: "a".repeat(64),
  projectId: "00000000-0000-4000-8000-000000000001",
  relayPubkey: "b".repeat(64),
};

const SUBMITTED = {
  problem: "why does this recur?",
  initialCoordinates: [],
  contextCoordinates: [],
};

function result(requestId, revision = 42, identity = IDENTITY) {
  return {
    ...identity,
    requestId,
    projectContextRevision: revision,
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
      omittedForResponseBudget: {
        automaticRoots: 0,
        paths: 0,
        summaries: 0,
      },
    },
    inputOutcomes: { initial: [], context: [] },
    roots: [],
    paths: [],
  };
}

function error(code, retryable = false) {
  return new SemanticProjectContextError({
    code,
    message: code,
    retryable,
  });
}

function start(state, token = nextSemanticAttemptToken(state)) {
  return semanticSessionReducer(state, {
    type: "run_started",
    token,
    submitted: SUBMITTED,
  });
}

function activate(state, token, requestId, overlay = { edgeKeys: ["edge-a"] }) {
  state = semanticSessionReducer(state, {
    type: "native_succeeded",
    token,
    result: result(requestId),
  });
  return semanticSessionReducer(state, {
    type: "pairing_observed",
    token,
    substrateRevision: 42,
    join: { status: "valid", overlay },
  });
}

test("native success waits for exact graph revision before atomically activating", () => {
  let state = start(createSemanticUiState(IDENTITY), 1);
  assert.equal(semanticQueryRequiresAllContext(state), true);
  state = semanticSessionReducer(state, {
    type: "native_succeeded",
    token: 1,
    result: result("request-a"),
  });
  assert.equal(state.attempt.status, "pairing");
  assert.equal(state.active, null);

  const waiting = semanticSessionReducer(state, {
    type: "pairing_observed",
    token: 1,
    substrateRevision: 41,
    join: { status: "valid", overlay: { edgeKeys: ["too-early"] } },
  });
  assert.equal(waiting, state);

  state = semanticSessionReducer(state, {
    type: "pairing_observed",
    token: 1,
    substrateRevision: 42,
    join: { status: "valid", overlay: { edgeKeys: ["edge-a"] } },
  });
  assert.equal(state.attempt.status, "idle");
  assert.equal(state.active.requestId, "request-a");
  assert.deepEqual(state.active.overlay, { edgeKeys: ["edge-a"] });
  assert.equal(semanticOverlayEligible(state, 42), true);
});

test("a new Run and transient failure retain the previous active snapshot", () => {
  let state = activate(start(createSemanticUiState(IDENTITY), 1), 1, "old");
  state = start(state, 2);
  assert.equal(state.active.requestId, "old");
  state = semanticSessionReducer(state, {
    type: "native_failed",
    token: 2,
    error: error("timeout", true),
  });
  assert.equal(state.attempt.status, "failed");
  assert.equal(state.active.requestId, "old");
});

test("restricted and verification failures clear active state", () => {
  for (const code of ["restricted", "verification_failed", "internal"]) {
    let state = activate(start(createSemanticUiState(IDENTITY), 1), 1, "old");
    state = start(state, 2);
    state = semanticSessionReducer(state, {
      type: "native_failed",
      token: 2,
      error: error(code),
    });
    assert.equal(state.active, null, code);
  }
});

test("Cancel invalidates in-flight and pairing responses", () => {
  let state = start(createSemanticUiState(IDENTITY), 1);
  state = semanticSessionReducer(state, { type: "cancel" });
  assert.equal(state.generation, 2);
  const afterLateNative = semanticSessionReducer(state, {
    type: "native_succeeded",
    token: 1,
    result: result("late"),
  });
  assert.equal(afterLateNative, state);

  state = start(state, 3);
  state = semanticSessionReducer(state, {
    type: "native_succeeded",
    token: 3,
    result: result("pairing"),
  });
  state = semanticSessionReducer(state, { type: "cancel" });
  const afterLatePairing = semanticSessionReducer(state, {
    type: "pairing_observed",
    token: 3,
    substrateRevision: 42,
    join: { status: "valid", overlay: { edgeKeys: ["late"] } },
  });
  assert.equal(afterLatePairing, state);
  assert.equal(afterLatePairing.active, null);
});

test("newer Run supersedes A and ignores A success or error", () => {
  let state = start(createSemanticUiState(IDENTITY), 1);
  state = start(state, 2);
  const afterA = semanticSessionReducer(state, {
    type: "native_succeeded",
    token: 1,
    result: result("request-a"),
  });
  assert.equal(afterA, state);
  const afterAError = semanticSessionReducer(state, {
    type: "native_failed",
    token: 1,
    error: error("timeout", true),
  });
  assert.equal(afterAError, state);
  assert.equal(state.attempt.token, 2);
});

test("workspace or caller change clears state and rejects the old token", () => {
  let state = createSemanticUiState(IDENTITY, {
    ...SUBMITTED,
    problem: "private old Community question",
  });
  state = activate(start(state, 1), 1, "old");
  state = start(state, 2);
  const identityB = {
    ...IDENTITY,
    appliedWorkspaceToken: "workspace-token-b",
    callerPubkey: "c".repeat(64),
  };
  state = semanticSessionReducer(state, {
    type: "boundary_changed",
    identity: identityB,
  });
  assert.equal(state.active, null);
  assert.equal(state.attempt.status, "idle");
  assert.equal(state.draft.problem, "");
  const afterLate = semanticSessionReducer(state, {
    type: "native_succeeded",
    token: 2,
    result: result("late"),
  });
  assert.equal(afterLate, state);
});

test("wrong response identity fails closed before pairing", () => {
  let state = start(createSemanticUiState(IDENTITY), 1);
  state = semanticSessionReducer(state, {
    type: "native_succeeded",
    token: 1,
    result: result("wrong", 42, {
      ...IDENTITY,
      callerPubkey: "c".repeat(64),
    }),
  });
  assert.equal(state.attempt.status, "failed");
  assert.equal(state.attempt.error.code, "verification_failed");
  assert.equal(state.active, null);
});

test("advanced graph suspends overlay while source and transport staleness stay orthogonal", () => {
  let state = activate(start(createSemanticUiState(IDENTITY), 1), 1, "active");
  const hinted = semanticSessionReducer(state, { type: "source_hint" });
  assert.equal(hinted, state);
  state = semanticSessionReducer(state, {
    type: "source_refresh_observed",
    fingerprintMatches: false,
  });
  state = semanticSessionReducer(state, {
    type: "transport_observed",
    state: "uncertain",
  });
  assert.equal(semanticSessionFreshness(state), "stale");
  assert.equal(semanticOverlayEligible(state, 42), true);

  state = semanticSessionReducer(state, {
    type: "topology_observed",
    substrateRevision: 43,
  });
  assert.equal(state.freshness.topology, "advanced");
  assert.equal(semanticOverlayEligible(state, 43), false);
  assert.equal(semanticSessionFreshness(state), "stale");
});

test("topology advance alone marks the retained snapshot stale", () => {
  let state = activate(start(createSemanticUiState(IDENTITY), 1), 1, "active");
  state = semanticSessionReducer(state, {
    type: "topology_observed",
    substrateRevision: 43,
  });
  assert.equal(semanticSessionFreshness(state), "stale");
  assert.equal(semanticOverlayEligible(state, 43), false);
});

test("pairing against an advanced or invalid graph fails but retains old active", () => {
  for (const action of [
    {
      type: "pairing_observed",
      token: 2,
      substrateRevision: 43,
      join: { status: "valid", overlay: { edgeKeys: ["new"] } },
    },
    {
      type: "pairing_observed",
      token: 2,
      substrateRevision: 42,
      join: { status: "invalid", message: "binding mismatch" },
    },
  ]) {
    let state = activate(start(createSemanticUiState(IDENTITY), 1), 1, "old");
    state = start(state, 2);
    state = semanticSessionReducer(state, {
      type: "native_succeeded",
      token: 2,
      result: result("new"),
    });
    state = semanticSessionReducer(state, action);
    assert.equal(state.attempt.status, "failed");
    assert.equal(state.attempt.error.code, "conflict");
    assert.equal(state.active.requestId, "old");
  }
});
