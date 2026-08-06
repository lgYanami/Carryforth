import assert from "node:assert/strict";
import test from "node:test";

import {
  MEETING_DIRECTORY_FALLBACK_INTERVAL_MS,
  MEETING_SNAPSHOT_FALLBACK_INTERVAL_MS,
  isTerminalMeetingLifecycle,
  meetingDirectoryFallbackInterval,
  meetingLiveSubscriptionIds,
  meetingSnapshotFallbackInterval,
} from "./meetingSyncPolicy.ts";

function item(meetingId, lifecycle, compatibility = "ready") {
  return { meetingId, lifecycle, compatibility };
}

test("terminal lifecycle classification covers close and abort only", () => {
  assert.equal(isTerminalMeetingLifecycle("closed"), true);
  assert.equal(isTerminalMeetingLifecycle("aborted"), true);
  assert.equal(isTerminalMeetingLifecycle("active"), false);
  assert.equal(isTerminalMeetingLifecycle("finalizing_actions"), false);
  assert.equal(isTerminalMeetingLifecycle(null), false);
});

test("directory fallback runs only while a readable Meeting is non-terminal", () => {
  assert.equal(meetingDirectoryFallbackInterval(undefined), false);
  assert.equal(
    meetingDirectoryFallbackInterval([
      item("closed", "closed"),
      item("aborted", "aborted"),
    ]),
    false,
  );
  assert.equal(
    meetingDirectoryFallbackInterval([item("active", "active")]),
    MEETING_DIRECTORY_FALLBACK_INTERVAL_MS,
  );
  assert.equal(
    meetingDirectoryFallbackInterval([
      item("finalizing", "finalizing_actions"),
    ]),
    MEETING_DIRECTORY_FALLBACK_INTERVAL_MS,
  );
  assert.equal(
    meetingDirectoryFallbackInterval([
      item("unsupported", null, "unsupported_protocol"),
    ]),
    false,
  );
});

test("selected snapshot fallback runs only for a ready non-terminal Meeting", () => {
  assert.equal(meetingSnapshotFallbackInterval(undefined), false);
  assert.equal(
    meetingSnapshotFallbackInterval({ status: "unsupported_relay" }),
    false,
  );
  assert.equal(
    meetingSnapshotFallbackInterval({
      status: "ready",
      snapshot: { lifecycle: "active" },
    }),
    MEETING_SNAPSHOT_FALLBACK_INTERVAL_MS,
  );
  assert.equal(
    meetingSnapshotFallbackInterval({
      status: "ready",
      snapshot: { lifecycle: "finalizing_actions" },
    }),
    MEETING_SNAPSHOT_FALLBACK_INTERVAL_MS,
  );
  assert.equal(
    meetingSnapshotFallbackInterval({
      status: "ready",
      snapshot: { lifecycle: "closed" },
    }),
    false,
  );
});

test("live subscriptions include unprojected rooms and exclude known terminal or unreadable rooms", () => {
  assert.deepEqual(
    meetingLiveSubscriptionIds(
      ["new", "active", "closed", "forbidden", "active"],
      [
        item("active", "active"),
        item("closed", "closed"),
        item("forbidden", null, "forbidden"),
      ],
    ),
    ["active", "new"],
  );
});
