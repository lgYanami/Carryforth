import assert from "node:assert/strict";
import test from "node:test";

import {
  ProjectContextInvalidationScheduler,
  projectContextInvalidationScopesForKind,
  projectContextLiveFilter,
} from "./liveSync.ts";
import {
  KIND_PROJECT_CONTEXT_EDGE_BINDING,
  KIND_PROJECT_CONTEXT_META,
  KIND_PROJECT_DOCUMENT_HEAD,
  KIND_PROJECT_DOCUMENT_META,
  KIND_PROJECT_VIEW_META,
  KIND_PROJECT_VIEW_OBJECT,
} from "../../shared/constants/kinds.ts";

test("combined live filter is signer-scoped and overlaps the oldest observation", () => {
  assert.deepEqual(
    projectContextLiveFilter({
      relayPubkey: "ABCD",
      contextUpdatedAt: "2026-08-06T08:00:12Z",
      projectViewUpdatedAt: "2026-08-06T08:00:10Z",
      documentUpdatedAt: "invalid",
      nowSeconds: 2_000_000_000,
    }),
    {
      authors: ["abcd"],
      kinds: [
        KIND_PROJECT_CONTEXT_EDGE_BINDING,
        KIND_PROJECT_CONTEXT_META,
        KIND_PROJECT_VIEW_OBJECT,
        KIND_PROJECT_VIEW_META,
        KIND_PROJECT_DOCUMENT_HEAD,
        KIND_PROJECT_DOCUMENT_META,
      ],
      limit: 512,
      since: Math.floor(Date.parse("2026-08-06T08:00:10Z") / 1_000) - 5,
    },
  );
});

test("combined live filter falls back to a current-time overlap", () => {
  assert.equal(
    projectContextLiveFilter({
      relayPubkey: "abcd",
      nowSeconds: 100,
    }).since,
    95,
  );
});

test("projection kinds invalidate only trusted source boundaries", () => {
  assert.deepEqual(
    projectContextInvalidationScopesForKind(KIND_PROJECT_CONTEXT_EDGE_BINDING),
    ["context"],
  );
  assert.deepEqual(
    projectContextInvalidationScopesForKind(KIND_PROJECT_VIEW_OBJECT),
    ["context", "project_view"],
  );
  assert.deepEqual(
    projectContextInvalidationScopesForKind(KIND_PROJECT_DOCUMENT_HEAD),
    ["context", "documents"],
  );
  assert.deepEqual(projectContextInvalidationScopesForKind(1), []);
});

test("bursts merge scopes and retain one trailing trusted refresh", async () => {
  const timers = new Map();
  const refreshes = [];
  let nextTimer = 1;
  let releaseFirst;
  const firstRefresh = new Promise((resolve) => {
    releaseFirst = resolve;
  });
  const scheduler = new ProjectContextInvalidationScheduler(
    async (scopes) => {
      refreshes.push([...scopes].sort());
      if (refreshes.length === 1) await firstRefresh;
    },
    10,
    (callback) => {
      const id = nextTimer++;
      timers.set(id, callback);
      return id;
    },
    (id) => timers.delete(id),
  );

  scheduler.signal("context");
  scheduler.signal(["documents", "context"]);
  assert.equal(timers.size, 1);
  const firstTimer = [...timers.values()][0];
  timers.clear();
  firstTimer();
  assert.deepEqual(refreshes, [["context", "documents"]]);

  scheduler.signal("project_view");
  scheduler.signal("documents");
  assert.equal(timers.size, 0);
  releaseFirst();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(timers.size, 1);
  const trailingTimer = [...timers.values()][0];
  timers.clear();
  trailingTimer();
  await Promise.resolve();
  assert.deepEqual(refreshes, [
    ["context", "documents"],
    ["documents", "project_view"],
  ]);
  scheduler.dispose();
});
