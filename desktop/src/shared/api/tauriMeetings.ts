import { invokeTauri } from "@/shared/api/tauri";

export type MeetingCapability = {
  status: "unsupported" | "readable" | "creatable";
  relayPubkey: string | null;
  supportsDirectActions: boolean;
  canCreateDirectActions: boolean;
};

export type CreateMeetingInput = {
  /** Stable UUID reused while an indeterminate signed Create is retried. */
  submissionId: string;
  title: string;
  description?: string;
  sourceChannelId?: string;
  /** Frozen roster excluding the current Human host. */
  participantPubkeys: string[];
  initialBoard: string;
};

export type CreateMeetingResult =
  | {
      status: "accepted";
      meetingId: string;
      eventId: string;
      hostPubkey: string;
      participantPubkeys: string[];
      title: string;
    }
  | {
      status: "indeterminate";
      meetingId: string;
      eventId: string;
      message: string;
    };

export type MeetingLifecycle =
  | "initializing"
  | "active"
  | "finalizing_actions"
  | "closed"
  | "aborted";

export type MeetingParticipant = {
  pubkey: string;
  participantType: "human" | "agent" | "unknown";
  channelRole: string;
};

export type MeetingBoard = {
  eventId: string;
  format: "markdown";
  body: string;
  moderatorPubkey: string;
  updatedAt: number;
  source: "projection" | "create";
};

export type MeetingActionState = {
  actionRunId: string;
  boardEventId: string;
  actionWindowEpoch: number;
  condition: "runnable" | "blocked" | string;
  terminalStatus: string | null;
  completionEventId: string | null;
  actionDeadlineAtMs: number | null;
  lastErrorCode: string | null;
};

export type MeetingEndState = {
  eventId: string;
  outcome: "closed" | "aborted";
  reasonCode: string | null;
  reason: string | null;
  endedBy: string;
  endedAt: number;
  actionsAttested: boolean;
};

export type MeetingSnapshot = {
  meetingId: string;
  title: string;
  description: string | null;
  sourceChannelId: string | null;
  schemaVersion: 3;
  policy: "moderated-board-v1" | "moderated-board-actions-v2";
  hostPubkey: string;
  moderatorPubkey: string;
  createEventId: string;
  createdAt: number;
  lifecycle: MeetingLifecycle;
  phase: string;
  stateRevision: number;
  floorRevision: number;
  intentRevision: number;
  speechRevision: number;
  currentSpeakerPubkey: string | null;
  currentOfferPubkey: string | null;
  participants: MeetingParticipant[];
  board: MeetingBoard;
  action: MeetingActionState | null;
  end: MeetingEndState | null;
  latestSpeechAt: number | null;
};

export type MeetingLoadResult =
  | { status: "unsupported_relay" }
  | { status: "forbidden" }
  | { status: "not_found" }
  | {
      status: "unsupported_protocol";
      meeting_id: string;
      schema_version: string | null;
      policy: string | null;
    }
  | { status: "ready"; snapshot: MeetingSnapshot };

export type MeetingListItem = {
  meetingId: string;
  title: string;
  lifecycle: MeetingLifecycle | null;
  phase: string | null;
  currentSpeakerPubkey: string | null;
  moderatorPubkey: string | null;
  policy: string | null;
  updatedAt: number | null;
  endedAt: number | null;
  latestSpeechAt: number | null;
  compatibility:
    | "ready"
    | "unsupported_relay"
    | "unsupported_protocol"
    | "forbidden"
    | "not_found";
};

export type MeetingSpeech = {
  eventId: string;
  authorPubkey: string;
  content: string;
  createdAt: number;
  speechRevision: number;
  grantEventId: string;
  mentions: string[];
};

export type MeetingSpeechCursor = {
  before: number;
  beforeId: string;
};

export type MeetingSpeechPage = {
  speeches: MeetingSpeech[];
  nextCursor: MeetingSpeechCursor | null;
};

export async function getMeetingCapability(): Promise<MeetingCapability> {
  return invokeTauri<MeetingCapability>("get_meeting_capability");
}

export async function createMeeting(
  input: CreateMeetingInput,
): Promise<CreateMeetingResult> {
  return invokeTauri<CreateMeetingResult>("create_meeting", { input });
}

export async function listMeetings(
  meetingIds: string[],
): Promise<MeetingListItem[]> {
  return invokeTauri<MeetingListItem[]>("list_meetings", { meetingIds });
}

export async function getMeetingSnapshot(
  meetingId: string,
): Promise<MeetingLoadResult> {
  return invokeTauri<MeetingLoadResult>("get_meeting_snapshot", { meetingId });
}

export async function getMeetingBoard(
  meetingId: string,
): Promise<MeetingLoadResult> {
  return invokeTauri<MeetingLoadResult>("get_meeting_board", { meetingId });
}

export async function getMeetingSpeeches(input: {
  meetingId: string;
  before?: MeetingSpeechCursor;
  limit?: number;
}): Promise<MeetingSpeechPage> {
  return invokeTauri<MeetingSpeechPage>("get_meeting_speeches", {
    meetingId: input.meetingId,
    before: input.before?.before ?? null,
    beforeId: input.before?.beforeId ?? null,
    limit: input.limit ?? null,
  });
}
