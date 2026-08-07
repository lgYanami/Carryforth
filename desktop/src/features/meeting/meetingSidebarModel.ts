import type { MeetingListItem } from "@/shared/api/tauriMeetings";

export function isTerminalMeeting(item: MeetingListItem): boolean {
  return item.lifecycle === "closed" || item.lifecycle === "aborted";
}

/** Community observers can discover Meetings but never inherit participant notifications. */
export function meetingCanNotifyViewer(item: MeetingListItem): boolean {
  return item.viewerRole === "host" || item.viewerRole === "participant";
}

export function terminalMeetingAttentionKey(
  item: MeetingListItem,
): string | null {
  if (!isTerminalMeeting(item) || !item.needsAttention) return null;
  return `${item.meetingId}:${item.attentionReason ?? "unknown"}:${item.endedAt ?? item.updatedAt ?? 0}`;
}

export function meetingNeedsVisibleAttention(
  item: MeetingListItem,
  acknowledgedTerminalAttention: ReadonlySet<string>,
): boolean {
  if (!item.needsAttention) return false;
  const terminalKey = terminalMeetingAttentionKey(item);
  return (
    terminalKey === null || !acknowledgedTerminalAttention.has(terminalKey)
  );
}

function compareRecent(left: MeetingListItem, right: MeetingListItem): number {
  return (
    (right.updatedAt ?? 0) - (left.updatedAt ?? 0) ||
    left.meetingId.localeCompare(right.meetingId)
  );
}

export function meetingSidebarItems(
  items: readonly MeetingListItem[],
  acknowledgedTerminalAttention: ReadonlySet<string>,
): { active: MeetingListItem[]; history: MeetingListItem[] } {
  const active = items
    .filter(
      (item) =>
        !isTerminalMeeting(item) ||
        meetingNeedsVisibleAttention(item, acknowledgedTerminalAttention),
    )
    .sort((left, right) => {
      const attention =
        Number(
          meetingNeedsVisibleAttention(right, acknowledgedTerminalAttention),
        ) -
        Number(
          meetingNeedsVisibleAttention(left, acknowledgedTerminalAttention),
        );
      if (attention !== 0) return attention;
      const activeState =
        Number(!isTerminalMeeting(right)) - Number(!isTerminalMeeting(left));
      return activeState || compareRecent(left, right);
    });
  const history = items
    .filter(isTerminalMeeting)
    .sort(
      (left, right) =>
        (right.endedAt ?? right.updatedAt ?? 0) -
          (left.endedAt ?? left.updatedAt ?? 0) ||
        left.meetingId.localeCompare(right.meetingId),
    );
  return { active, history };
}
