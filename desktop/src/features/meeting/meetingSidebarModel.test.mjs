import assert from "node:assert/strict";
import test from "node:test";

import {
  meetingCanNotifyViewer,
  meetingNeedsVisibleAttention,
  meetingSidebarItems,
  terminalMeetingAttentionKey,
} from "./meetingSidebarModel.ts";

function item({
  id,
  lifecycle = "active",
  needsAttention = false,
  attentionReason = null,
  updatedAt = 0,
  endedAt = null,
  viewerRole = "participant",
}) {
  return {
    meetingId: id,
    title: id,
    description: null,
    lifecycle,
    phase: "moderator_control",
    currentSpeakerPubkey: null,
    currentOfferPubkey: null,
    needsAttention,
    attentionReason,
    moderatorPubkey: null,
    hostPubkey: null,
    participantCount: 0,
    participantPreview: [],
    viewerRole,
    policy: "moderated-board-actions-v3",
    createdAt: 0,
    updatedAt,
    endedAt,
    latestSpeechAt: null,
    compatibility: "ready",
  };
}

test("Community observers never inherit roster notification semantics", () => {
  assert.equal(
    meetingCanNotifyViewer(item({ id: "host", viewerRole: "host" })),
    true,
  );
  assert.equal(
    meetingCanNotifyViewer(
      item({ id: "participant", viewerRole: "participant" }),
    ),
    true,
  );
  assert.equal(
    meetingCanNotifyViewer(item({ id: "observer", viewerRole: "observer" })),
    false,
  );
  assert.equal(
    meetingCanNotifyViewer(item({ id: "unknown", viewerRole: null })),
    false,
  );
});

test("Meeting sidebar sorts attention, active state, recent activity, then stable ID", () => {
  const meetings = [
    item({ id: "active-new", updatedAt: 40 }),
    item({
      id: "attention-old",
      needsAttention: true,
      attentionReason: "floor_offer",
      updatedAt: 10,
    }),
    item({ id: "active-a", updatedAt: 20 }),
    item({ id: "active-b", updatedAt: 20 }),
    item({ id: "closed", lifecycle: "closed", updatedAt: 50, endedAt: 50 }),
  ];

  assert.deepEqual(
    meetingSidebarItems(meetings, new Set()).active.map(
      (meeting) => meeting.meetingId,
    ),
    ["attention-old", "active-new", "active-a", "active-b"],
  );
});

test("aborted attention remains visible until its exact terminal state is acknowledged", () => {
  const aborted = item({
    id: "aborted",
    lifecycle: "aborted",
    needsAttention: true,
    attentionReason: "meeting_aborted",
    updatedAt: 30,
    endedAt: 30,
  });
  const key = terminalMeetingAttentionKey(aborted);
  assert.ok(key);
  assert.equal(meetingNeedsVisibleAttention(aborted, new Set()), true);
  assert.deepEqual(meetingSidebarItems([aborted], new Set()).active, [aborted]);

  const acknowledged = new Set([key]);
  assert.equal(meetingNeedsVisibleAttention(aborted, acknowledged), false);
  assert.deepEqual(meetingSidebarItems([aborted], acknowledged).active, []);
  assert.deepEqual(meetingSidebarItems([aborted], acknowledged).history, [
    aborted,
  ]);

  const newerAbort = { ...aborted, endedAt: 31, updatedAt: 31 };
  assert.equal(meetingNeedsVisibleAttention(newerAbort, acknowledged), true);
});
