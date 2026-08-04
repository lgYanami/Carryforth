import assert from "node:assert/strict";
import test from "node:test";

import { fromRawChannel } from "./tauriChannels.ts";

function rawChannel(room_kind) {
  return {
    id: "00000000-0000-4000-8000-000000000001",
    name: "Meeting-looking title",
    channel_type: "stream",
    room_kind,
    visibility: "private",
    description: "",
    topic: null,
    purpose: null,
    member_count: 2,
    member_pubkeys: [],
    last_message_at: null,
    archived_at: null,
    participants: [],
    participant_pubkeys: [],
    is_member: true,
    ttl_seconds: null,
    ttl_deadline: null,
  };
}

test("recognizes only the exact meeting room discriminator", () => {
  assert.equal(fromRawChannel(rawChannel("meeting")).roomKind, "meeting");
});

test("missing, normal, and unknown room discriminators stay ordinary", () => {
  assert.equal(fromRawChannel(rawChannel(undefined)).roomKind, null);
  assert.equal(fromRawChannel(rawChannel("channel")).roomKind, null);
  assert.equal(fromRawChannel(rawChannel("future-room")).roomKind, null);
});
