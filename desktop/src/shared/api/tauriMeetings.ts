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

export type MeetingHumanFloorRequest = {
  requestId: string;
  requesterPubkey: string;
  queuePosition: number;
  state: "queued" | "offered";
};

export type MeetingHandoffContext = {
  fromPubkey: string;
  reasonType:
    | "question"
    | "information_request"
    | "clarification"
    | "review"
    | "response_requested";
  reasonText: string;
};

export type MeetingOffer = {
  offerId: string;
  targetPubkey: string;
  targetParticipantType: "human" | "agent";
  allocationSource:
    | "human_request"
    | "fallback"
    | "moderator_select"
    | "directed_handoff";
  turnRole: "participant" | "moderator_self";
  selectionReason: string | null;
  sourceIntentId: string | null;
  sourceRequestId: string | null;
  sourceHandoffId: string | null;
  sourceSpeechEventId: string | null;
  handoffContext: MeetingHandoffContext | null;
  createdAtMs: number;
  ackDeadlineMs: number;
};

export type MeetingGrant = {
  grantId: string;
  holderPubkey: string;
  allocationSource: MeetingOffer["allocationSource"];
  turnRole: MeetingOffer["turnRole"];
  selectionReason: string | null;
  sourceIntentId: string | null;
  sourceRequestId: string | null;
  sourceHandoffId: string | null;
  sourceSpeechEventId: string | null;
  handoffContext: MeetingHandoffContext | null;
  createdAtMs: number;
  softLeaseExpiresAtMs: number;
  hardDeadlineMs: number;
  progressSeq: number;
};

export type MeetingFloorState = {
  /** Opaque Relay-authored concurrency token. */
  stateEventId: string;
  humanQueue: MeetingHumanFloorRequest[];
  offer: MeetingOffer | null;
  grant: MeetingGrant | null;
};

export type MeetingPendingIntent = {
  intentId: string;
  currentEventId: string;
  authorPubkey: string;
  basisSpeechRevision: number;
  summary: string;
  addressedTo: string | null;
  createdAtMs: number;
  deferred: boolean;
  selectionAttemptCount: number;
  lastOfferId: string | null;
  lastAttemptOutcome: string | null;
  eligibleDecisionEpoch: number;
  selectable: boolean;
};

export type MeetingOpenHandoff = {
  handoffId: string;
  sourceSpeechEventId: string;
  fromPubkey: string;
  toPubkey: string;
  reasonType: MeetingHandoffType;
  reasonText: string;
  createdAtMs: number;
  attemptCount: number;
  lastOfferId: string | null;
  lastGrantId: string | null;
  lastAttemptOutcome: string | null;
  blockedBy: string | null;
  moderatorRetryBlocked: boolean;
  eligibleDecisionEpoch: number;
  attemptActive: boolean;
  selectable: boolean;
};

export type MeetingBoardControl = {
  phase:
    | "bootstrap_locked"
    | "board_pending"
    | "floor_ready"
    | "finalizing_actions"
    | "ended";
  controlEpoch: number;
  boardWindow: number;
  boardStartedAtMs: number | null;
  boardDeadlineAtMs: number | null;
  boardCompletedAtMs: number | null;
  boardOutcome: "updated" | "unchanged" | "timed_out" | "preempted" | null;
};

export type MeetingHostState = {
  /** Opaque native-issued concurrency token; never interpreted by React. */
  controlToken: string;
  stateEventId: string;
  controlEpoch: number;
  decisionEpoch: number;
  decisionDeadlineMs: number | null;
  nextActionAtMs: number | null;
  consecutiveModeratorSpeeches: number;
  forcedReturnToModerator: boolean;
  pendingIntents: MeetingPendingIntent[];
  openHandoffs: MeetingOpenHandoff[];
  boardControl: MeetingBoardControl;
  canSelect: boolean;
  canClose: boolean;
  canRecall: boolean;
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
  floor: MeetingFloorState | null;
  host: MeetingHostState | null;
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
  currentOfferPubkey: string | null;
  humanFloorAttentionPubkey: string | null;
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

export type MeetingActivityKind =
  | "board_updated"
  | "board_unchanged"
  | "board_timed_out"
  | "board_preempted"
  | "floor_offered"
  | "floor_granted"
  | "offer_declined"
  | "offer_expired"
  | "floor_yielded"
  | "floor_recalled"
  | "floor_expired"
  | "handoff_opened"
  | "handoff_attempted"
  | "handoff_resolved"
  | "action_finalization_started"
  | "action_blocked"
  | "action_retried"
  | "action_returned_to_board"
  | "action_deadline_exceeded"
  | "meeting_closed"
  | "meeting_aborted";

export type MeetingActivity = {
  /** Stable opaque identity; never interpreted as a Relay event ID. */
  activityId: string;
  kind: MeetingActivityKind;
  occurredAtMs: number;
  actorPubkey: string | null;
  targetPubkey: string | null;
  summary: string;
};

export type MeetingActivityPage = {
  activities: MeetingActivity[];
  /** Opaque native cursor; React must only pass it back unchanged. */
  nextCursor: string | null;
};

export type MeetingHandoffType =
  | "question"
  | "information_request"
  | "clarification"
  | "review"
  | "response_requested";

export type MeetingGrantYieldReason =
  | "no_longer_needed"
  | "unable_to_answer"
  | "insufficient_context"
  | "tool_failure"
  | "cancelled";

export type MeetingFloorAction =
  | { type: "request" }
  | { type: "withdraw" }
  | { type: "offer_ack" }
  | { type: "offer_decline"; reason?: string }
  | {
      type: "grant_yield";
      reasonCode?: MeetingGrantYieldReason;
      reason?: string;
    }
  | {
      type: "speech";
      content: string;
      mentions: string[];
      handoff?: {
        targetPubkey: string;
        handoffType: MeetingHandoffType;
        reason: string;
      };
    };

export type MeetingFloorActionInput = {
  /** Stable UUID reused while an indeterminate signed command is retried. */
  submissionId: string;
  meetingId: string;
  expectedStateEventId: string;
  action: MeetingFloorAction;
};

export type MeetingFloorActionResult =
  | {
      status: "accepted";
      meetingId: string;
      eventId: string;
      action: string;
      canonicalObjectId: string | null;
      stateRevision: number | null;
      duplicate: boolean;
    }
  | {
      status: "indeterminate";
      meetingId: string;
      eventId: string;
      action: string;
      message: string;
    };

export type MeetingIntentRejectionReason =
  | "off_topic"
  | "duplicate"
  | "superseded"
  | "unsupported"
  | "agenda_mismatch";

export type MeetingHandoffDismissReason =
  | "superseded"
  | "answered_elsewhere"
  | "out_of_scope"
  | "no_longer_needed";

export type MeetingAbortReason =
  | "goal_unreachable"
  | "insufficient_information"
  | "discussion_blocked"
  | "unable_to_form_conclusion"
  | "moderator_unable_to_continue";

export type MeetingHostAction =
  | { type: "board_update"; body: string }
  | { type: "board_unchanged" }
  | { type: "intent_submit"; summary: string; addressedTo?: string }
  | {
      type: "intent_refresh";
      intentId: string;
      summary: string;
      addressedTo?: string;
    }
  | { type: "intent_withdraw"; intentId: string }
  | {
      type: "select_intent";
      intentId: string;
      selectionReason?: string;
      deferralReason?: string;
    }
  | {
      type: "select_handoff";
      handoffId: string;
      selectionReason?: string;
    }
  | {
      type: "reject_intent";
      intentId: string;
      reasonCode: MeetingIntentRejectionReason;
      reason: string;
    }
  | {
      type: "dismiss_handoff";
      handoffId: string;
      reasonCode: MeetingHandoffDismissReason;
      reason: string;
    }
  | { type: "recall"; reason?: string }
  | { type: "close" }
  | { type: "abort"; reasonCode: MeetingAbortReason; reason?: string };

export type MeetingHostActionInput = {
  /** Stable UUID reused while an indeterminate signed command is retried. */
  submissionId: string;
  meetingId: string;
  expectedControlToken: string;
  action: MeetingHostAction;
};

export type MeetingHostActionResult =
  | {
      status: "accepted";
      meetingId: string;
      eventId: string;
      action: string;
      canonicalObjectId: string | null;
      stateRevision: number | null;
      duplicate: boolean;
    }
  | {
      status: "indeterminate";
      meetingId: string;
      eventId: string;
      action: string;
      message: string;
    };

export type MeetingActionBlockReason =
  | "external_operation_failed"
  | "external_state_conflict"
  | "tool_unavailable"
  | "provider_failure"
  | "affinity_lost"
  | "action_deadline_exceeded";

export type MeetingActionFinalizationAction =
  | { type: "begin" }
  | {
      type: "block";
      reasonCode: MeetingActionBlockReason;
      reason?: string;
    }
  | { type: "retry" }
  | { type: "return_to_board" }
  | { type: "confirm" };

export type MeetingActionFinalizationInput = {
  /** Stable UUID reused while an indeterminate signed command is retried. */
  submissionId: string;
  meetingId: string;
  /** Opaque token binding the current discussion or action window. */
  expectedControlToken: string;
  action: MeetingActionFinalizationAction;
};

export type MeetingActionFinalizationResult =
  | {
      status: "accepted";
      meetingId: string;
      eventId: string;
      action: string;
      stateRevision: number | null;
      duplicate: boolean;
    }
  | {
      status: "indeterminate";
      meetingId: string;
      eventId: string;
      action: string;
      message: string;
    };

export async function getMeetingCapability(): Promise<MeetingCapability> {
  return invokeTauri<MeetingCapability>("get_meeting_capability");
}

export async function createMeeting(
  input: CreateMeetingInput,
): Promise<CreateMeetingResult> {
  return invokeTauri<CreateMeetingResult>("create_meeting", { input });
}

export async function submitMeetingFloorAction(
  input: MeetingFloorActionInput,
): Promise<MeetingFloorActionResult> {
  return invokeTauri<MeetingFloorActionResult>("submit_meeting_floor_action", {
    input,
  });
}

export async function submitMeetingHostAction(
  input: MeetingHostActionInput,
): Promise<MeetingHostActionResult> {
  return invokeTauri<MeetingHostActionResult>("submit_meeting_host_action", {
    input,
  });
}

export async function submitMeetingActionFinalization(
  input: MeetingActionFinalizationInput,
): Promise<MeetingActionFinalizationResult> {
  return invokeTauri<MeetingActionFinalizationResult>(
    "submit_meeting_action_finalization",
    { input },
  );
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

export async function getMeetingActivities(input: {
  meetingId: string;
  cursor?: string;
  limit?: number;
}): Promise<MeetingActivityPage> {
  return invokeTauri<MeetingActivityPage>("get_meeting_activities", {
    meetingId: input.meetingId,
    cursor: input.cursor ?? null,
    limit: input.limit ?? null,
  });
}
