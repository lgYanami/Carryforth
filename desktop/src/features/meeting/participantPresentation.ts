import type {
  MeetingParticipant,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";

export type MeetingParticipantStatusKind =
  | "speaking"
  | "waiting_for_ack"
  | "floor_requested"
  | "intent_pending"
  | "idle";

export type MeetingParticipantStatus = {
  kind: MeetingParticipantStatusKind;
  label: string;
  detail: string | null;
};

export type MeetingParticipantPresentation = {
  participant: MeetingParticipant;
  isHost: boolean;
  status: MeetingParticipantStatus;
};

export type MeetingParticipantGroup = {
  key: "host" | "human" | "agent" | "unknown";
  label: string;
  participants: MeetingParticipantPresentation[];
};

function samePubkey(left: string | null | undefined, right: string): boolean {
  return left?.toLowerCase() === right.toLowerCase();
}

/**
 * Derive one stable primary status from the authoritative Meeting snapshot.
 * Higher-priority states intentionally hide lower-priority ones so a participant
 * is never presented as being in two conflicting floor states at once.
 */
export function meetingParticipantStatus(
  participant: MeetingParticipant,
  snapshot: MeetingSnapshot,
): MeetingParticipantStatus {
  const pubkey = participant.pubkey;
  const grantHolder =
    snapshot.floor?.grant?.holderPubkey ?? snapshot.currentSpeakerPubkey;
  if (samePubkey(grantHolder, pubkey)) {
    return { kind: "speaking", label: "Speaking", detail: null };
  }

  const offerTarget =
    snapshot.floor?.offer?.targetPubkey ?? snapshot.currentOfferPubkey;
  if (samePubkey(offerTarget, pubkey)) {
    return {
      kind: "waiting_for_ack",
      label: "Waiting for ACK",
      detail: null,
    };
  }

  const request = snapshot.floor?.humanQueue.find((candidate) =>
    samePubkey(candidate.requesterPubkey, pubkey),
  );
  if (request) {
    return {
      kind: "floor_requested",
      label: request.state === "offered" ? "Floor offered" : "Floor requested",
      detail: `Queue ${request.queuePosition}`,
    };
  }

  const intent = snapshot.host?.pendingIntents.find((candidate) =>
    samePubkey(candidate.authorPubkey, pubkey),
  );
  if (intent) {
    return {
      kind: "intent_pending",
      label: "Intent pending",
      detail: intent.deferred ? "Deferred" : null,
    };
  }

  return { kind: "idle", label: "Idle", detail: null };
}

/** Group the frozen roster without duplicating the immutable host. */
export function meetingParticipantGroups(
  snapshot: MeetingSnapshot,
): MeetingParticipantGroup[] {
  const presentations = snapshot.participants
    .map((participant) => ({
      participant,
      isHost: samePubkey(snapshot.moderatorPubkey, participant.pubkey),
      status: meetingParticipantStatus(participant, snapshot),
    }))
    .sort((left, right) =>
      left.participant.pubkey.localeCompare(right.participant.pubkey),
    );
  const host = presentations.filter((participant) => participant.isHost);
  const remaining = presentations.filter((participant) => !participant.isHost);
  const groups: MeetingParticipantGroup[] = [
    { key: "host", label: "Host", participants: host },
    {
      key: "human",
      label: "Human participants",
      participants: remaining.filter(
        ({ participant }) => participant.participantType === "human",
      ),
    },
    {
      key: "agent",
      label: "Agent participants",
      participants: remaining.filter(
        ({ participant }) => participant.participantType === "agent",
      ),
    },
    {
      key: "unknown",
      label: "Pending classification",
      participants: remaining.filter(
        ({ participant }) => participant.participantType === "unknown",
      ),
    },
  ];
  return groups.filter((group) => group.participants.length > 0);
}
