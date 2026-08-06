import assert from "node:assert/strict";
import test from "node:test";

import {
  MEETING_DIRECTORY_FALLBACK_INTERVAL_MS,
  isTerminalMeetingLifecycle,
  meetingDirectoryFallbackInterval,
} from "./meetingSyncPolicy.ts";

function item(lifecycle, compatibility = "ready") {
  return { lifecycle, compatibility };
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
    meetingDirectoryFallbackInterval([item("closed"), item("aborted")]),
    false,
  );
  assert.equal(
    meetingDirectoryFallbackInterval([item("active")]),
    MEETING_DIRECTORY_FALLBACK_INTERVAL_MS,
  );
  assert.equal(
    meetingDirectoryFallbackInterval([item("finalizing_actions")]),
    MEETING_DIRECTORY_FALLBACK_INTERVAL_MS,
  );
  assert.equal(
    meetingDirectoryFallbackInterval([item(null, "unsupported_protocol")]),
    false,
  );
});
