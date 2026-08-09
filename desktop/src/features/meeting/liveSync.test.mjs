import assert from "node:assert/strict";
import test from "node:test";

import {
  MeetingLiveInvalidationScheduler,
  MeetingLiveSubscriptionManager,
  meetingLiveFilter,
} from "./liveSync.ts";

const STATE_KIND = 42103;
const END_KIND = 42101;
const SPEECH_KIND = 9;
const SUMMARY_KIND = 42113;

async function flushPromises() {
  await Promise.resolve();
  await Promise.resolve();
}

function fakeTimers() {
  let nextId = 1;
  const callbacks = new Map();
  return {
    setTimeoutFn(callback) {
      const id = nextId++;
      callbacks.set(id, callback);
      return id;
    },
    clearTimeoutFn(id) {
      callbacks.delete(id);
    },
    runNext() {
      const entry = callbacks.entries().next().value;
      assert.ok(entry, "expected a scheduled callback");
      const [id, callback] = entry;
      callbacks.delete(id);
      callback();
    },
    get size() {
      return callbacks.size;
    },
  };
}

function relayEvent(kind) {
  return { kind };
}

test("Meeting live filters always carry exactly one channel scope", () => {
  assert.deepEqual(meetingLiveFilter(" meeting-a ", 100), {
    kinds: [SPEECH_KIND, STATE_KIND, END_KIND, SUMMARY_KIND],
    "#h": ["meeting-a"],
    limit: 256,
    since: 95,
  });
  assert.throws(() => meetingLiveFilter("", 100), /one channel ID/);
});

test("subscription manager opens one REQ per Meeting and diffs terminal removal", async () => {
  const timers = fakeTimers();
  const subscriptions = [];
  const disposed = [];
  const signals = [];
  const manager = new MeetingLiveSubscriptionManager({
    subscribe: async (filter, onEvent) => {
      const meetingId = filter["#h"][0];
      subscriptions.push({ meetingId, filter, onEvent });
      return async () => {
        disposed.push(meetingId);
      };
    },
    onSignal: (meetingId, signal) => signals.push([meetingId, signal]),
    nowSeconds: () => 100,
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
  });

  manager.sync(["meeting-a", "meeting-b", "meeting-c"]);
  await flushPromises();

  assert.equal(subscriptions.length, 3);
  assert.deepEqual(
    subscriptions.map(({ filter }) => filter["#h"]),
    [["meeting-a"], ["meeting-b"], ["meeting-c"]],
  );
  assert.deepEqual(signals, [
    ["meeting-a", "initial"],
    ["meeting-b", "initial"],
    ["meeting-c", "initial"],
  ]);

  subscriptions[1].onEvent(relayEvent(STATE_KIND));
  assert.deepEqual(signals.at(-1), ["meeting-b", STATE_KIND]);

  manager.sync(["meeting-a", "meeting-c"]);
  await flushPromises();
  assert.deepEqual(disposed, ["meeting-b"]);
  assert.equal(subscriptions.length, 3, "healthy subscriptions are retained");

  manager.destroy();
  await flushPromises();
  assert.deepEqual(disposed.sort(), ["meeting-a", "meeting-b", "meeting-c"]);
});

test("one failed Meeting retries without replacing healthy subscriptions", async () => {
  const timers = fakeTimers();
  const attempts = new Map();
  const errors = [];
  const manager = new MeetingLiveSubscriptionManager({
    subscribe: async (filter) => {
      const meetingId = filter["#h"][0];
      const attempt = (attempts.get(meetingId) ?? 0) + 1;
      attempts.set(meetingId, attempt);
      if (meetingId === "meeting-b" && attempt === 1) {
        throw new Error("temporary failure");
      }
      return async () => {};
    },
    onSignal: () => {},
    onError: (meetingId, _error, retryInMs) =>
      errors.push({ meetingId, retryInMs }),
    nowSeconds: () => 100,
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
  });

  manager.sync(["meeting-a", "meeting-b"]);
  await flushPromises();
  assert.deepEqual(Object.fromEntries(attempts), {
    "meeting-a": 1,
    "meeting-b": 1,
  });
  assert.deepEqual(errors, [{ meetingId: "meeting-b", retryInMs: 500 }]);
  assert.equal(timers.size, 1);

  timers.runNext();
  await flushPromises();
  assert.deepEqual(Object.fromEntries(attempts), {
    "meeting-a": 1,
    "meeting-b": 2,
  });
  manager.destroy();
});

test("late subscription completion is disposed after the Meeting leaves scope", async () => {
  const timers = fakeTimers();
  let resolveSubscription;
  let disposed = 0;
  const signals = [];
  const manager = new MeetingLiveSubscriptionManager({
    subscribe: () =>
      new Promise((resolve) => {
        resolveSubscription = resolve;
      }),
    onSignal: (...signal) => signals.push(signal),
    setTimeoutFn: timers.setTimeoutFn,
    clearTimeoutFn: timers.clearTimeoutFn,
  });

  manager.sync(["meeting-a"]);
  manager.sync([]);
  resolveSubscription(async () => {
    disposed += 1;
  });
  await flushPromises();

  assert.equal(disposed, 1);
  assert.deepEqual(signals, []);
  manager.destroy();
});

test("invalidation scheduler coalesces bursts and preserves a trailing refresh", async () => {
  const timers = fakeTimers();
  const refreshes = [];
  let finishFirst;
  const scheduler = new MeetingLiveInvalidationScheduler(
    async (signals) => {
      refreshes.push([...signals]);
      if (refreshes.length === 1) {
        await new Promise((resolve) => {
          finishFirst = resolve;
        });
      }
    },
    150,
    timers.setTimeoutFn,
    timers.clearTimeoutFn,
  );

  scheduler.signal(STATE_KIND);
  scheduler.signal(SPEECH_KIND);
  assert.equal(timers.size, 1);
  timers.runNext();
  await flushPromises();
  assert.deepEqual(new Set(refreshes[0]), new Set([STATE_KIND, SPEECH_KIND]));

  scheduler.signal(END_KIND);
  assert.equal(timers.size, 0, "running refresh retains a trailing signal");
  finishFirst();
  await flushPromises();
  assert.equal(timers.size, 1);
  timers.runNext();
  await flushPromises();
  assert.deepEqual(refreshes[1], [END_KIND]);

  scheduler.dispose();
});
