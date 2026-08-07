import assert from "node:assert/strict";
import test from "node:test";

import {
  ALL_PROJECT_CONTEXT_QUERY,
  isIncompatibleProjectContextCacheEntry,
  projectContextCacheKey,
  projectContextCommunityKey,
  projectContextRelayOrigin,
  projectContextResultIdentity,
} from "./hooks.ts";
import {
  projectContextFailureKind,
  visibleContextDocumentCount,
} from "./state.ts";
import { ProjectContextError } from "../../shared/api/tauriProjectContext.ts";

const result = {
  communityKey: "community-a-0",
  projectId: "00000000-0000-4000-8000-000000000001",
  relayPubkey: "a".repeat(64),
  context: {
    contextRevision: 4,
    projectionGeneration: 2,
    activeEdgeCount: 2,
    boundDocumentCount: 3,
    updatedAt: "2026-08-06T08:00:00Z",
    metaEventId: "b".repeat(64),
    capabilityEnabled: true,
  },
  query: ALL_PROJECT_CONTEXT_QUERY,
  projectViewObservation: { state: "not_requested" },
  documentObservation: { state: "not_requested" },
  edges: [
    {
      edgeKey: "c".repeat(64),
      coordinateKeys: ["requirement:a", "resource:b"],
      contextDocumentIds: ["doc-a", "doc-b"],
    },
    {
      edgeKey: "d".repeat(64),
      coordinateKeys: ["requirement:a", "document:c"],
      contextDocumentIds: ["doc-b"],
    },
  ],
  coordinateDetails: [],
  documentDetails: [],
};

test("default descriptor is exactly empty contains-all", () => {
  assert.deepEqual(ALL_PROJECT_CONTEXT_QUERY, {
    type: "contains_all",
    coordinates: [],
  });
  assert.deepEqual(
    projectContextCacheKey("community-a-0", "https://relay.example", {
      type: "contains_all",
      coordinates: [],
    }),
    [
      "project-context",
      "community-a-0",
      "https://relay.example",
      '{"type":"contains_all","coordinates":[]}',
    ],
  );
});

test("community and Relay cache scopes include reinit and canonical origin", () => {
  assert.equal(
    projectContextCommunityKey({ communityId: "alpha", reinitKey: 3 }),
    "alpha-3",
  );
  assert.equal(
    projectContextRelayOrigin("wss://Relay.Example/path"),
    "https://relay.example",
  );
});

test("only stale identities from the same Community cache are removed", () => {
  const identity = projectContextResultIdentity(result);
  assert.equal(
    isIncompatibleProjectContextCacheEntry({
      queryKey: ["project-context", "community-a-0"],
      data: result,
      communityKey: "community-a-0",
      identity,
    }),
    false,
  );
  assert.equal(
    isIncompatibleProjectContextCacheEntry({
      queryKey: ["project-context", "community-a-0"],
      data: {
        ...result,
        context: { ...result.context, projectionGeneration: 1 },
      },
      communityKey: "community-a-0",
      identity,
    }),
    true,
  );
  assert.equal(
    isIncompatibleProjectContextCacheEntry({
      queryKey: ["project-context", "community-b-0"],
      data: result,
      communityKey: "community-a-0",
      identity,
    }),
    false,
  );
});

test("screen failures retain closed structured distinctions", () => {
  for (const code of [
    "unsupported",
    "restricted",
    "unavailable",
    "snapshot_conflict",
    "verification_failed",
  ]) {
    assert.equal(
      projectContextFailureKind(
        new ProjectContextError({
          code,
          message: code,
          retryable: code === "unavailable" || code === "snapshot_conflict",
        }),
      ),
      code,
    );
  }
  assert.equal(projectContextFailureKind(new Error("network")), "error");
});

test("visible Context Document counts deduplicate cross-edge bindings", () => {
  assert.equal(visibleContextDocumentCount(result), 2);
});
