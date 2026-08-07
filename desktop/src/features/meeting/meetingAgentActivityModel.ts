import {
  buildChannelAgentSessionCandidates,
  type ChannelAgentSessionAgent,
} from "@/features/channels/ui/useChannelAgentSessions";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { ManagedAgent, RelayAgent } from "@/shared/api/types";
import type {
  MeetingLifecycle,
  MeetingParticipant,
} from "@/shared/api/tauriMeetings";
import { normalizePubkey, truncatePubkey } from "@/shared/lib/pubkey";

export type MeetingAgentActivityAgent = ChannelAgentSessionAgent;

type BuildMeetingAgentActivityAgentsInput = {
  currentPubkey?: string | null;
  managedAgents: readonly ManagedAgent[];
  participants: readonly MeetingParticipant[];
  profiles: UserProfileLookup;
  relayAgents: readonly RelayAgent[];
};

function profileName(pubkey: string, profiles: UserProfileLookup): string {
  const profile = profiles[normalizePubkey(pubkey)];
  return (
    profile?.displayName?.trim() ||
    profile?.name?.trim() ||
    truncatePubkey(pubkey)
  );
}

/**
 * Resolve the frozen Meeting roster into Agent Activity candidates visible to
 * the current Desktop identity. Community Agent registries only supply
 * presentation/runtime metadata; they never expand the frozen roster.
 */
export function buildMeetingAgentActivityAgents({
  currentPubkey,
  managedAgents,
  participants,
  profiles,
  relayAgents,
}: BuildMeetingAgentActivityAgentsInput): MeetingAgentActivityAgent[] {
  const normalizedCurrentPubkey = currentPubkey
    ? normalizePubkey(currentPubkey)
    : null;
  const managedPubkeys = new Set(
    managedAgents.map((agent) => normalizePubkey(agent.pubkey)),
  );
  const candidates = buildChannelAgentSessionCandidates({
    managedAgents: [...managedAgents],
    relayAgents: [...relayAgents],
  });
  const candidatesByPubkey = new Map(
    candidates.map((agent) => [normalizePubkey(agent.pubkey), agent]),
  );
  const seen = new Set<string>();
  const result: MeetingAgentActivityAgent[] = [];

  for (const participant of participants) {
    if (participant.participantType !== "agent") {
      continue;
    }

    const normalizedParticipantPubkey = normalizePubkey(participant.pubkey);
    if (seen.has(normalizedParticipantPubkey)) {
      continue;
    }
    seen.add(normalizedParticipantPubkey);

    const profile = profiles[normalizedParticipantPubkey];
    const viewerOwnsAgent = Boolean(
      normalizedCurrentPubkey &&
        profile?.ownerPubkey &&
        normalizePubkey(profile.ownerPubkey) === normalizedCurrentPubkey,
    );
    if (!managedPubkeys.has(normalizedParticipantPubkey) && !viewerOwnsAgent) {
      continue;
    }

    const candidate = candidatesByPubkey.get(normalizedParticipantPubkey);
    result.push(
      candidate
        ? {
            ...candidate,
            // Meeting Activity is observation-only. Meeting turns are fenced
            // by Floor/Host/Action state and are not interrupted from here.
            canInterruptTurn: false,
          }
        : {
            agentSource: "member-bot",
            canInterruptTurn: false,
            name: profileName(participant.pubkey, profiles),
            pubkey: participant.pubkey,
            status: "deployed",
          },
    );
  }

  return result;
}

export function selectWorkingMeetingAgents({
  agents,
  lifecycle,
  workingPubkeys,
}: {
  agents: readonly MeetingAgentActivityAgent[];
  lifecycle: MeetingLifecycle;
  workingPubkeys: readonly string[];
}): MeetingAgentActivityAgent[] {
  if (lifecycle === "closed" || lifecycle === "aborted") {
    return [];
  }

  const working = new Set(workingPubkeys.map(normalizePubkey));
  return agents.filter((agent) => working.has(normalizePubkey(agent.pubkey)));
}
