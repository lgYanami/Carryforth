import type { RelayAgent, UserSearchResult } from "@/shared/api/types";
import { normalizePubkey, truncatePubkey } from "@/shared/lib/pubkey";

export const MEETING_ACTION_CAPABILITY = "meeting-v2-action-finalization-v4";
export const MAX_MEETING_BOARD_BYTES = 65_536;
export const MAX_MEETING_PARTICIPANTS = 12;
export const MAX_OTHER_MEETING_PARTICIPANTS = MAX_MEETING_PARTICIPANTS - 1;
export const MAX_MEETING_AGENTS = 8;

export type MeetingAgentCapability =
  | "not_applicable"
  | "compatible"
  | "incompatible"
  | "unknown";

export type MeetingRosterCandidate = UserSearchResult & {
  actionCapability: MeetingAgentCapability;
};

export type InitialMeetingBoardInput = {
  title: string;
  goal: string;
  agenda: readonly string[];
  background: string;
  references: string;
};

export type MeetingDraftValidationInput = {
  title: string;
  goal: string;
  participants: readonly Pick<UserSearchResult, "isAgent" | "pubkey">[];
  board: string;
};

export type MeetingSourceAccess =
  | { status: "ok"; missingPubkeys: [] }
  | { status: "loading"; missingPubkeys: [] }
  | { status: "unavailable"; missingPubkeys: [] }
  | { status: "blocked"; missingPubkeys: string[] };

/** Translate the Relay's stable roster-capability rejection into named UI copy. */
export function describeMeetingCapabilityRejection(
  message: string,
  candidates: readonly Pick<UserSearchResult, "displayName" | "pubkey">[],
): string | null {
  if (!message.includes("restricted:meeting:roster_capability_missing")) {
    return null;
  }
  const encoded = message.match(/missing_agent_pubkeys=([0-9a-f,]+)/iu)?.[1];
  if (!encoded) {
    return "The Relay rejected this roster because at least one Agent no longer advertises the required Meeting capability. Refresh and try again.";
  }
  const byPubkey = new Map(
    candidates.map((candidate) => [
      normalizePubkey(candidate.pubkey),
      candidate,
    ]),
  );
  const names = encoded
    .split(",")
    .map(normalizePubkey)
    .map(
      (pubkey) => byPubkey.get(pubkey)?.displayName || truncatePubkey(pubkey),
    );
  return `The Relay rejected this roster because ${names.join(", ")} ${names.length === 1 ? "no longer advertises" : "no longer advertise"} the required Meeting capability. Refresh and try again.`;
}

function nonemptyLines(value: string): string[] {
  return value
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter(Boolean);
}

/** Deterministically generate the complete initial free-Markdown Board. */
export function buildInitialMeetingBoard(
  input: InitialMeetingBoardInput,
): string {
  const title = input.title.trim() || "Meeting";
  const goal = input.goal.trim();
  const agenda = input.agenda.map((item) => item.trim()).filter(Boolean);
  const background = input.background.trim();
  const references = nonemptyLines(input.references);
  const sections = [`# ${title}`, `## Discussion goal\n\n${goal}`];

  if (agenda.length > 0) {
    sections.push(
      `## Agenda\n\n${agenda
        .map((item, index) => `${index + 1}. ${item}`)
        .join("\n")}`,
    );
  }
  if (background) {
    sections.push(`## Background and context\n\n${background}`);
  }
  if (references.length > 0) {
    sections.push(
      `## References\n\n${references.map((reference) => `- ${reference}`).join("\n")}`,
    );
  }

  return `${sections.join("\n\n")}\n`;
}

export function validateMeetingDraft(
  input: MeetingDraftValidationInput,
): string[] {
  const errors: string[] = [];
  const title = input.title.trim();
  if (!title) {
    errors.push("Meeting name is required.");
  } else if ([...title].length > 255) {
    errors.push("Meeting name must be 255 characters or fewer.");
  }
  if (!input.goal.trim()) {
    errors.push("Discussion goal is required.");
  }

  const normalizedParticipants = input.participants.map((participant) =>
    normalizePubkey(participant.pubkey),
  );
  if (
    normalizedParticipants.length < 1 ||
    normalizedParticipants.length > MAX_OTHER_MEETING_PARTICIPANTS
  ) {
    errors.push(
      "Choose between 1 and 11 participants in addition to yourself.",
    );
  }
  if (new Set(normalizedParticipants).size !== normalizedParticipants.length) {
    errors.push("The Meeting roster contains a duplicate participant.");
  }
  if (
    normalizedParticipants.some((pubkey) => !/^[0-9a-f]{64}$/u.test(pubkey))
  ) {
    errors.push("Every Meeting participant must have a valid public key.");
  }
  if (
    input.participants.filter((participant) => participant.isAgent).length >
    MAX_MEETING_AGENTS
  ) {
    errors.push(`A Meeting can include at most ${MAX_MEETING_AGENTS} Agents.`);
  }

  if (!input.board.trim()) {
    errors.push("The initial Board is required.");
  }
  if (input.board.includes("\0")) {
    errors.push("The initial Board cannot contain NUL characters.");
  }
  const boardBytes = new TextEncoder().encode(input.board).byteLength;
  if (boardBytes > MAX_MEETING_BOARD_BYTES) {
    errors.push(
      `The initial Board exceeds the ${MAX_MEETING_BOARD_BYTES.toLocaleString()} byte limit.`,
    );
  }
  return errors;
}

export function classifyMeetingAgentCapability(
  candidate: Pick<UserSearchResult, "isAgent" | "pubkey">,
  relayAgents: readonly Pick<RelayAgent, "capabilities" | "pubkey">[],
): MeetingAgentCapability {
  if (!candidate.isAgent) return "not_applicable";
  const pubkey = normalizePubkey(candidate.pubkey);
  const relayAgent = relayAgents.find(
    (agent) => normalizePubkey(agent.pubkey) === pubkey,
  );
  if (!relayAgent) return "unknown";
  return relayAgent.capabilities.includes(MEETING_ACTION_CAPABILITY)
    ? "compatible"
    : "incompatible";
}

/** Merge directory sources by identity while preserving an Agent classification. */
export function dedupeMeetingRosterCandidates(
  candidates: readonly UserSearchResult[],
  relayAgents: readonly Pick<RelayAgent, "capabilities" | "pubkey">[],
): MeetingRosterCandidate[] {
  const byPubkey = new Map<string, UserSearchResult>();
  for (const candidate of candidates) {
    const pubkey = normalizePubkey(candidate.pubkey);
    const current = byPubkey.get(pubkey);
    byPubkey.set(pubkey, {
      pubkey,
      displayName: current?.displayName ?? candidate.displayName,
      avatarUrl: current?.avatarUrl ?? candidate.avatarUrl,
      nip05Handle: current?.nip05Handle ?? candidate.nip05Handle,
      ownerPubkey: current?.ownerPubkey ?? candidate.ownerPubkey,
      isAgent: Boolean(current?.isAgent || candidate.isAgent),
    });
  }
  return [...byPubkey.values()].map((candidate) => ({
    ...candidate,
    actionCapability: classifyMeetingAgentCapability(candidate, relayAgents),
  }));
}

/** Product preflight for the optional source Channel navigation reference. */
export function checkMeetingSourceAccess(input: {
  sourceVisibility: "open" | "private" | null;
  rosterPubkeys: readonly string[];
  memberPubkeys?: readonly string[];
  membersLoading?: boolean;
  membersUnavailable?: boolean;
}): MeetingSourceAccess {
  if (input.sourceVisibility === null || input.sourceVisibility === "open") {
    return { status: "ok", missingPubkeys: [] };
  }
  if (input.membersUnavailable) {
    return { status: "unavailable", missingPubkeys: [] };
  }
  if (input.membersLoading || input.memberPubkeys === undefined) {
    return { status: "loading", missingPubkeys: [] };
  }
  const members = new Set(input.memberPubkeys.map(normalizePubkey));
  const missingPubkeys = [
    ...new Set(input.rosterPubkeys.map(normalizePubkey)),
  ].filter((pubkey) => !members.has(pubkey));
  return missingPubkeys.length > 0
    ? { status: "blocked", missingPubkeys }
    : { status: "ok", missingPubkeys: [] };
}
