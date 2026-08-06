import type {
  MeetingLifecycle,
  MeetingListItem,
} from "@/shared/api/tauriMeetings";

export const MEETING_DIRECTORY_FALLBACK_INTERVAL_MS = 12_000;

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
