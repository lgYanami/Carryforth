import assert from "node:assert/strict";
import test from "node:test";

import {
  clearMeetingPendingCommand,
  readMeetingPendingCommand,
  writeMeetingPendingCommand,
} from "./meetingPendingCommandStore.ts";

class MemoryStorage {
  #values = new Map();

  get length() {
    return this.#values.size;
  }

  clear() {
    this.#values.clear();
  }

  getItem(key) {
    return this.#values.get(key) ?? null;
  }

  key(index) {
    return [...this.#values.keys()][index] ?? null;
  }

  removeItem(key) {
    this.#values.delete(key);
  }

  setItem(key, value) {
    this.#values.set(key, String(value));
  }
}

test("pending Meeting commands are isolated by scope, lane, and Meeting", () => {
  const previousWindow = globalThis.window;
  const sessionStorage = new MemoryStorage();
  globalThis.window = { sessionStorage };
  try {
    const meetingA = "00000000-0000-4000-8000-000000000001";
    const meetingB = "00000000-0000-4000-8000-000000000002";
    const scopeA = `community:alice:${meetingA}`;
    const scopeB = `community:alice:${meetingB}`;
    const hostA = {
      action: { intentId: "11".repeat(32), type: "select_intent" },
      expectedControlToken: "22".repeat(32),
      meetingId: meetingA,
      submissionId: "00000000-0000-4000-8000-000000000010",
    };
    const hostB = {
      action: { type: "close" },
      expectedControlToken: "33".repeat(32),
      meetingId: meetingB,
      submissionId: "00000000-0000-4000-8000-000000000011",
    };
    const floorA = {
      action: { type: "request" },
      expectedStateEventId: "44".repeat(32),
      meetingId: meetingA,
      submissionId: "00000000-0000-4000-8000-000000000012",
    };

    writeMeetingPendingCommand(scopeA, "host", hostA);
    writeMeetingPendingCommand(scopeB, "host", hostB);
    writeMeetingPendingCommand(scopeA, "floor", floorA);

    assert.deepEqual(
      readMeetingPendingCommand(scopeA, "host", meetingA),
      hostA,
    );
    assert.deepEqual(
      readMeetingPendingCommand(scopeB, "host", meetingB),
      hostB,
    );
    assert.deepEqual(
      readMeetingPendingCommand(scopeA, "floor", meetingA),
      floorA,
    );
    assert.equal(readMeetingPendingCommand(scopeA, "host", meetingB), null);

    clearMeetingPendingCommand(scopeA, "host");
    assert.equal(readMeetingPendingCommand(scopeA, "host", meetingA), null);
    assert.deepEqual(
      readMeetingPendingCommand(scopeB, "host", meetingB),
      hostB,
    );
  } finally {
    if (previousWindow === undefined) {
      delete globalThis.window;
    } else {
      globalThis.window = previousWindow;
    }
  }
});
