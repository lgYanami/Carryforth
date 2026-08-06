import type {
  MeetingLifecycle,
  MeetingListItem,
  MeetingLoadResult,
} from "@/shared/api/tauriMeetings";

export const MEETING_DIRECTORY_FALLBACK_INTERVAL_MS = 12_000;
export const MEETING_SNAPSHOT_FALLBACK_INTERVAL_MS = 12_000;

export function isTerminalMeetingLifecycle(
  lifecycle: MeetingLifecycle | null | undefined,
): boolean {
  return lifecycle === "closed" || lifecycle === "aborted";
}

/**
 * Keep a low-frequency canonical reread only while at least one readable
 * Meeting is non-terminal. Unsupported/forbidden entries cannot converge by
 * polling and are intentionally excluded.
 */
export function meetingDirectoryFallbackInterval(
  meetings: readonly MeetingListItem[] | undefined,
): number | false {
  return meetings?.some(
    (meeting) =>
      meeting.compatibility === "ready" &&
      !isTerminalMeetingLifecycle(meeting.lifecycle),
  )
    ? MEETING_DIRECTORY_FALLBACK_INTERVAL_MS
    : false;
}

/**
 * Keep the selected canonical snapshot converging while a Meeting can still
 * change. Live events remain the low-latency path; this is the bounded recovery
 * path for a missed or silently mis-scoped subscription.
 */
export function meetingSnapshotFallbackInterval(
  result: MeetingLoadResult | undefined,
): number | false {
  return result?.status === "ready" &&
    !isTerminalMeetingLifecycle(result.snapshot.lifecycle)
    ? MEETING_SNAPSHOT_FALLBACK_INTERVAL_MS
    : false;
}

/**
 * Resolve the Meeting rooms that need a live channel subscription.
 *
 * A newly discovered room is subscribed before its directory projection
 * exists, avoiding a discovery deadlock. Once a canonical directory result is
 * available, terminal and unreadable Meetings are removed deterministically.
 */
export function meetingLiveSubscriptionIds(
  roomIds: readonly string[],
  meetings: readonly MeetingListItem[] | undefined,
): string[] {
  const projected = new Map(
    (meetings ?? []).map((meeting) => [meeting.meetingId, meeting]),
  );

  return [...new Set(roomIds)]
    .filter((meetingId) => {
      const meeting = projected.get(meetingId);
      if (!meeting) return true;
      return (
        meeting.compatibility === "ready" &&
        !isTerminalMeetingLifecycle(meeting.lifecycle)
      );
    })
    .sort();
}
