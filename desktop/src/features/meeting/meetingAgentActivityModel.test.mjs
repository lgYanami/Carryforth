import assert from "node:assert/strict";
import test from "node:test";

import {
  buildMeetingAgentActivityAgents,
  selectWorkingMeetingAgents,
} from "./meetingAgentActivityModel.ts";

const OWNER = "a".repeat(64);
const OTHER_OWNER = "b".repeat(64);
const MANAGED_AGENT = "c".repeat(64);
const RELAY_AGENT = "d".repeat(64);
const UNOWNED_AGENT = "e".repeat(64);
const NON_ROSTER_AGENT = "f".repeat(64);
const HUMAN = "1".repeat(64);

function participant(pubkey, participantType = "agent") {
  return { pubkey, participantType, channelRole: "member" };
}

function managedAgent(pubkey = MANAGED_AGENT) {
  return {
    pubkey,
    name: "Managed roster agent",
    status: "running",
  };
}

function relayAgent(pubkey, name, status = "online") {
  return {
    pubkey,
    name,
    status,
    agentType: "codex",
    channels: [],
    channelIds: [],
    capabilities: [],
    respondTo: null,
    respondToAllowlist: [],
  };
}

test("builds an owner-visible Agent list from the frozen Meeting roster", () => {
  const agents = buildMeetingAgentActivityAgents({
    currentPubkey: OWNER,
    participants: [
      participant(HUMAN, "human"),
      participant(MANAGED_AGENT),
      participant(RELAY_AGENT.toUpperCase()),
      participant(UNOWNED_AGENT),
      participant(RELAY_AGENT),
    ],
    managedAgents: [managedAgent()],
    relayAgents: [
      relayAgent(MANAGED_AGENT, "Stale relay name", "offline"),
      relayAgent(RELAY_AGENT, "Owned relay agent"),
      relayAgent(UNOWNED_AGENT, "Someone else's agent"),
      relayAgent(NON_ROSTER_AGENT, "Not in this Meeting"),
    ],
    profiles: {
      [RELAY_AGENT]: {
        displayName: "Profile relay name",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: OWNER,
      },
      [UNOWNED_AGENT]: {
        displayName: "Unowned",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: OTHER_OWNER,
      },
    },
  });

  assert.deepEqual(
    agents.map((agent) => ({
      canInterruptTurn: agent.canInterruptTurn,
      name: agent.name,
      pubkey: agent.pubkey.toLowerCase(),
      source: agent.agentSource,
      status: agent.status,
    })),
    [
      {
        canInterruptTurn: false,
        name: "Managed roster agent",
        pubkey: MANAGED_AGENT,
        source: "managed",
        status: "running",
      },
      {
        canInterruptTurn: false,
        name: "Owned relay agent",
        pubkey: RELAY_AGENT,
        source: "relay",
        status: "deployed",
      },
    ],
  );
});

test("uses profile metadata for an owned frozen Agent missing registry metadata", () => {
  const agents = buildMeetingAgentActivityAgents({
    currentPubkey: OWNER,
    participants: [participant(RELAY_AGENT)],
    managedAgents: [],
    relayAgents: [],
    profiles: {
      [RELAY_AGENT]: {
        displayName: "Profile-only Agent",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: OWNER,
      },
    },
  });

  assert.deepEqual(agents, [
    {
      agentSource: "member-bot",
      canInterruptTurn: false,
      name: "Profile-only Agent",
      pubkey: RELAY_AGENT,
      status: "deployed",
    },
  ]);
});

test("selects working Agents in frozen roster order and suppresses terminal stale state", () => {
  const agents = buildMeetingAgentActivityAgents({
    currentPubkey: OWNER,
    participants: [participant(MANAGED_AGENT), participant(RELAY_AGENT)],
    managedAgents: [managedAgent()],
    relayAgents: [relayAgent(RELAY_AGENT, "Relay Agent")],
    profiles: {
      [RELAY_AGENT]: {
        displayName: "Relay Agent",
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: OWNER,
      },
    },
  });

  const working = selectWorkingMeetingAgents({
    agents,
    lifecycle: "active",
    workingPubkeys: [
      RELAY_AGENT.toUpperCase(),
      MANAGED_AGENT,
      RELAY_AGENT,
      NON_ROSTER_AGENT,
    ],
  });
  assert.deepEqual(
    working.map((agent) => agent.pubkey.toLowerCase()),
    [MANAGED_AGENT, RELAY_AGENT],
  );

  assert.deepEqual(
    selectWorkingMeetingAgents({
      agents,
      lifecycle: "closed",
      workingPubkeys: [MANAGED_AGENT, RELAY_AGENT],
    }),
    [],
  );
});
