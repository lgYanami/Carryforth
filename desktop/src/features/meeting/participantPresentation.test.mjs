import assert from "node:assert/strict";
import test from "node:test";

import {
  meetingParticipantGroups,
  meetingParticipantStatus,
} from "./participantPresentation.ts";

const HOST = "1".repeat(64);
const HUMAN = "2".repeat(64);
const AGENT = "3".repeat(64);
const UNKNOWN = "4".repeat(64);

function snapshot() {
  return {
    moderatorPubkey: HOST,
    currentSpeakerPubkey: AGENT,
    currentOfferPubkey: HUMAN,
    participants: [
      { pubkey: HUMAN, participantType: "human", channelRole: "member" },
      { pubkey: HOST, participantType: "agent", channelRole: "owner" },
      { pubkey: UNKNOWN, participantType: "unknown", channelRole: "member" },
      { pubkey: AGENT, participantType: "agent", channelRole: "bot" },
    ],
    floor: {
      humanQueue: [
        {
          requestId: "5".repeat(64),
          requesterPubkey: HUMAN,
          queuePosition: 2,
          state: "queued",
        },
      ],
      offer: { targetPubkey: HUMAN },
      grant: { holderPubkey: AGENT },
    },
    host: {
      pendingIntents: [
        { authorPubkey: HOST, deferred: false },
        { authorPubkey: AGENT, deferred: true },
      ],
    },
  };
}

test("groups the frozen roster without duplicating the host or guessing unknown identity", () => {
  const groups = meetingParticipantGroups(snapshot());

  assert.deepEqual(
    groups.map((group) => [group.key, group.participants.length]),
    [
      ["host", 1],
      ["human", 1],
      ["agent", 1],
      ["unknown", 1],
    ],
  );
  assert.equal(groups[0].participants[0].participant.pubkey, HOST);
  assert.equal(
    groups.flatMap((group) => group.participants).filter((item) => item.isHost)
      .length,
    1,
  );
});

test("uses the fixed Speaking, Offer, Request, Intent, Idle priority", () => {
  const value = snapshot();
  const byPubkey = Object.fromEntries(
    value.participants.map((participant) => [participant.pubkey, participant]),
  );

  assert.equal(
    meetingParticipantStatus(byPubkey[AGENT], value).kind,
    "speaking",
  );
  assert.equal(
    meetingParticipantStatus(byPubkey[HUMAN], value).kind,
    "waiting_for_ack",
  );
  assert.equal(
    meetingParticipantStatus(byPubkey[HOST], value).kind,
    "intent_pending",
  );
  assert.equal(meetingParticipantStatus(byPubkey[UNKNOWN], value).kind, "idle");

  value.floor.offer = null;
  value.currentOfferPubkey = null;
  const request = meetingParticipantStatus(byPubkey[HUMAN], value);
  assert.equal(request.kind, "floor_requested");
  assert.equal(request.detail, "Queue 2");
});
