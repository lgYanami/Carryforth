import assert from "node:assert/strict";
import test from "node:test";

import {
  projectViewLiveFilter,
  ProjectViewInvalidationScheduler,
} from "./liveSync.ts";
import {
  KIND_PROJECT_VIEW_META,
  KIND_PROJECT_VIEW_OBJECT,
} from "../../shared/constants/kinds.ts";

test("Project View live filter is signer-scoped and overlaps the verified snapshot", () => {
  assert.deepEqual(
    projectViewLiveFilter({
      relayPubkey: "ABCD",
      snapshotUpdatedAt: "2026-07-28T08:00:10Z",
      nowSeconds: 2_000_000_000,
    }),
    {
      authors: ["abcd"],
      kinds: [KIND_PROJECT_VIEW_OBJECT, KIND_PROJECT_VIEW_META],
      limit: 256,
      since: Math.floor(Date.parse("2026-07-28T08:00:10Z") / 1_000) - 5,
    },
  );
});

test("Project View live filter falls back to a current-time overlap", () => {
  assert.equal(
    projectViewLiveFilter({
      relayPubkey: "abcd",
      nowSeconds: 100,
    }).since,
    95,
  );
});

test("projection bursts coalesce and retain one signal during refresh", async () => {
  const timers = new Map();
  let nextTimer = 1;
  let refreshCount = 0;
  let releaseRefresh;
  const firstRefresh = new Promise((resolve) => {
    releaseRefresh = resolve;
  });
  const scheduler = new ProjectViewInvalidationScheduler(
    async () => {
      refreshCount += 1;
      if (refreshCount === 1) await firstRefresh;
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
  const firstTimer = [...timers.values()][0];
  timers.clear();
  firstTimer();
  assert.equal(refreshCount, 1);

  scheduler.signal();
  scheduler.signal();
  assert.equal(timers.size, 0);
  releaseRefresh();
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(timers.size, 1);

  const trailingTimer = [...timers.values()][0];
  timers.clear();
  trailingTimer();
  await Promise.resolve();
  assert.equal(refreshCount, 2);
  scheduler.dispose();
});

test("a failed snapshot refresh does not stop the next projection signal", async () => {
  const timers = [];
  let refreshCount = 0;
  const scheduler = new ProjectViewInvalidationScheduler(
    async () => {
      refreshCount += 1;
      if (refreshCount === 1) throw new Error("snapshot race");
    },
    10,
    (callback) => {
      timers.push(callback);
      return timers.length;
    },
    () => {},
  );

  scheduler.signal();
  timers.shift()?.();
  await Promise.resolve();
  await Promise.resolve();
  scheduler.signal();
  timers.shift()?.();
  await Promise.resolve();
  assert.equal(refreshCount, 2);
  scheduler.dispose();
});
