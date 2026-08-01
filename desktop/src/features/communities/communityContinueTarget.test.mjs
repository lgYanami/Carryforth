import assert from "node:assert/strict";
import test from "node:test";

import { resolveCommunityContinueTarget } from "./communityContinueTarget.ts";

function channel(overrides = {}) {
  return {
    id: "general-id",
    name: "general",
    channelType: "stream",
    archivedAt: null,
    isMember: true,
    ...overrides,
  };
}

test("defaults to Inbox without a remembered channel", () => {
  assert.deepEqual(resolveCommunityContinueTarget(null, [], false), {
    status: "ready",
    target: { kind: "home", label: "Open Inbox" },
  });
});

test("does not trust a cached channel before live validation", () => {
  assert.deepEqual(
    resolveCommunityContinueTarget(
      { kind: "channel", channelId: "general-id" },
      [channel()],
      false,
    ),
    {
      status: "pending",
      target: { kind: "home", label: "Open Inbox" },
    },
  );
});

test("continues only into a joined active channel", () => {
  assert.deepEqual(
    resolveCommunityContinueTarget(
      { kind: "channel", channelId: "general-id" },
      [channel()],
      true,
    ),
    {
      status: "ready",
      target: {
        kind: "channel",
        channelId: "general-id",
        label: "Continue in #general",
      },
    },
  );

  for (const unavailable of [
    channel({ isMember: false }),
    channel({ archivedAt: "2026-07-30T00:00:00Z" }),
    channel({ id: "other-id" }),
  ]) {
    assert.equal(
      resolveCommunityContinueTarget(
        { kind: "channel", channelId: "general-id" },
        [unavailable],
        true,
      ).status,
      "invalid",
    );
  }
});

test("uses a conversation label for direct messages", () => {
  const resolution = resolveCommunityContinueTarget(
    { kind: "channel", channelId: "dm-id" },
    [
      channel({
        id: "dm-id",
        name: "alice",
        channelType: "dm",
      }),
    ],
    true,
  );

  assert.deepEqual(resolution.target, {
    kind: "channel",
    channelId: "dm-id",
    label: "Continue alice",
  });
});
