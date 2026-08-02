import assert from "node:assert/strict";
import test from "node:test";

import {
  projectDocumentHistoryKey,
  projectDocumentKey,
  projectDocumentMetaKey,
  projectDocumentsKey,
} from "./hooks.ts";
import {
  ProjectDocumentInvalidationScheduler,
  projectDocumentLiveFilter,
} from "./liveSync.ts";
import { diffDocumentLines } from "./lineDiff.ts";
import {
  KIND_PROJECT_DOCUMENT_HEAD,
  KIND_PROJECT_DOCUMENT_META,
} from "../../shared/constants/kinds.ts";

const identity = {
  communityKey: "community-a-0",
  projectId: "project-a",
  relayPubkey: "ab".repeat(32),
  projectionGeneration: 4,
};
const meta = {
  ...identity,
  catalogRevision: 9,
  activeDocumentCount: 2,
  updatedAt: "2026-07-30T08:00:00Z",
  metaEventId: "cd".repeat(32),
};

test("Document query keys pin every authority and keep current apart from pinned", () => {
  assert.deepEqual(projectDocumentMetaKey("community-a-0", "https://relay"), [
    "project-document-meta",
    "community-a-0",
    "https://relay",
  ]);
  assert.deepEqual(projectDocumentsKey(meta), [
    "project-documents",
    "community-a-0",
    "project-a",
    "ab".repeat(32),
    4,
    9,
  ]);
  assert.notDeepEqual(
    projectDocumentKey(identity, "document-a", "current"),
    projectDocumentKey(identity, "document-a", 2),
  );
  assert.deepEqual(projectDocumentHistoryKey(identity, "document-a", 7), [
    "project-document-history",
    "community-a-0",
    "project-a",
    "ab".repeat(32),
    4,
    "document-a",
    7,
  ]);
});

test("live filter is signer-scoped and carries no event-body parsing surface", () => {
  assert.deepEqual(
    projectDocumentLiveFilter({
      relayPubkey: "ABCD",
      snapshotUpdatedAt: "2026-07-30T08:00:10Z",
    }),
    {
      authors: ["abcd"],
      kinds: [KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META],
      limit: 256,
      since: Math.floor(Date.parse("2026-07-30T08:00:10Z") / 1_000) - 5,
    },
  );
});

test("Document invalidation bursts coalesce with one trailing refresh", async () => {
  const timers = new Map();
  let nextTimer = 1;
  let refreshes = 0;
  let release;
  const pending = new Promise((resolve) => {
    release = resolve;
  });
  const scheduler = new ProjectDocumentInvalidationScheduler(
    async () => {
      refreshes += 1;
      if (refreshes === 1) await pending;
    },
    10,
    (callback) => {
      const id = nextTimer++;
      timers.set(id, callback);
      return id;
    },
    (id) => timers.delete(id),
  );
  scheduler.signal();
  scheduler.signal();
  assert.equal(timers.size, 1);
  const first = [...timers.values()][0];
  timers.clear();
  first();
  scheduler.signal();
  release();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(timers.size, 1);
  const trailing = [...timers.values()][0];
  timers.clear();
  trailing();
  await Promise.resolve();
  assert.equal(refreshes, 2);
  scheduler.dispose();
});

test("exact diff never applies fuzzy offsets", () => {
  const rows = diffDocumentLines("alpha\nbeta\n", "inserted\nalpha\ngamma\n");
  assert.deepEqual(
    rows.map(({ kind, text }) => [kind, text]),
    [
      ["insert", "inserted"],
      ["context", "alpha"],
      ["delete", "beta"],
      ["insert", "gamma"],
      ["context", ""],
    ],
  );
});

test("exact diff preserves a terminal newline change", () => {
  const rows = diffDocumentLines("alpha", "alpha\n");
  assert.deepEqual(
    rows.map(({ kind, text }) => [kind, text]),
    [
      ["context", "alpha"],
      ["insert", ""],
    ],
  );
});
