import type {
  MeetingOpenHandoff,
  MeetingPendingIntent,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";

export type AgentHostPhasePresentation = {
  kind:
    | "board_maintenance"
    | "floor_decision"
    | "offer"
    | "grant"
    | "action_finalization"
    | "complete";
  title: string;
  description: string;
  deadlineMs: number | null;
};

export function agentHostPhasePresentation(
  snapshot: MeetingSnapshot,
): AgentHostPhasePresentation {
  if (snapshot.lifecycle === "closed" || snapshot.lifecycle === "aborted") {
    return {
      kind: "complete",
      title:
        snapshot.lifecycle === "closed"
          ? "Meeting completed"
          : "Meeting aborted",
      description:
        "The final Board and formal Speech remain available as the meeting record.",
      deadlineMs: null,
    };
  }
  if (snapshot.lifecycle === "finalizing_actions") {
    return {
      kind: "action_finalization",
      title: "Action finalization",
      description:
        snapshot.action?.condition === "blocked"
          ? "The Agent host's action-recording window is blocked and needs recovery."
          : "The Agent host is recording the frozen final Board in the relevant systems.",
      deadlineMs:
        snapshot.action?.condition === "runnable"
          ? (snapshot.action.actionDeadlineAtMs ?? null)
          : null,
    };
  }
  if (snapshot.host?.boardControl.phase === "board_pending") {
    return {
      kind: "board_maintenance",
      title: "Board maintenance",
      description:
        "The Agent host is reviewing or updating the Board before the next Floor decision.",
      deadlineMs: snapshot.host.boardControl.boardDeadlineAtMs,
    };
  }
  if (snapshot.floor?.grant || snapshot.currentSpeakerPubkey) {
    return {
      kind: "grant",
      title: "Formal Speech in progress",
      description:
        "The current Grant holder has the Floor. The host cannot interrupt a formal Speech.",
      deadlineMs: snapshot.floor?.grant?.hardDeadlineMs ?? null,
    };
  }
  if (snapshot.floor?.offer || snapshot.currentOfferPubkey) {
    return {
      kind: "offer",
      title: "Waiting for Floor acknowledgement",
      description:
        "The selected participant must accept or decline before a Grant can exist.",
      deadlineMs: snapshot.floor?.offer?.ackDeadlineMs ?? null,
    };
  }
  return {
    kind: "floor_decision",
    title: "Floor decision",
    description:
      "The Agent host is deciding which eligible Intent or Handoff should receive the next Offer.",
    deadlineMs: snapshot.host?.decisionDeadlineMs ?? null,
  };
}

export function agentHostBoardOutcomeLabel(snapshot: MeetingSnapshot): string {
  const outcome = snapshot.host?.boardControl.boardOutcome;
  switch (outcome) {
    case "updated":
      return "Board updated";
    case "unchanged":
      return "Board confirmed unchanged";
    case "timed_out":
      return "Board window timed out";
    case "preempted":
      return "Board maintenance preempted by Floor priority";
    default:
      return snapshot.host?.boardControl.phase === "board_pending"
        ? "Board review in progress"
        : "No completed Board outcome yet";
  }
}

export function agentHostIntentStatus(
  intent: MeetingPendingIntent,
  snapshot: MeetingSnapshot,
): string {
  if (intent.deferred) return "Deferred";
  if (intent.lastAttemptOutcome) {
    return `Last attempt: ${intent.lastAttemptOutcome.replaceAll("_", " ")}`;
  }
  if (intent.eligibleDecisionEpoch > (snapshot.host?.decisionEpoch ?? 0)) {
    return "Waiting for the next decision";
  }
  return intent.selectable ? "Ready for host decision" : "Pending";
}

export function agentHostHandoffStatus(handoff: MeetingOpenHandoff): string {
  if (handoff.attemptActive) return "Active Offer or Grant";
  if (handoff.moderatorRetryBlocked) return "Retry blocked";
  if (handoff.blockedBy) return "Blocked by another decision";
  if (handoff.lastAttemptOutcome) {
    return `Last attempt: ${handoff.lastAttemptOutcome.replaceAll("_", " ")}`;
  }
  return handoff.selectable ? "Ready for host decision" : "Open";
}

export function agentHostActionStatus(
  snapshot: MeetingSnapshot,
): string | null {
  const action = snapshot.action;
  if (!action) return null;
  switch (action.terminalStatus) {
    case "returned_to_board":
      return "Returned to Board maintenance";
    case "completed_closed":
      return "Actions confirmed and Meeting closed";
    case "completed_aborted":
      return "Action finalization ended with an abort";
    default:
      return action.condition === "blocked"
        ? "Action recording is blocked"
        : "Ready to record actions";
  }
}
