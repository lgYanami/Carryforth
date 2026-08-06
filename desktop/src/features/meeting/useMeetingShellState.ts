import * as React from "react";

import type { Channel } from "@/shared/api/types";
import type { MeetingListItem } from "@/shared/api/tauriMeetings";
import { useMeetingDirectory, useMeetingLiveSync } from "./hooks";

/** Partition protocol-specific Meeting rooms away from ordinary chat surfaces. */
export function useMeetingRoomPartition(channels: Channel[]) {
  const conversationChannels = React.useMemo(
    () => channels.filter((channel) => channel.roomKind !== "meeting"),
    [channels],
  );
  const memberChannels = React.useMemo(
    () => channels.filter((channel) => channel.isMember),
    [channels],
  );
  const meetingRooms = React.useMemo(
    () => memberChannels.filter((channel) => channel.roomKind === "meeting"),
    [memberChannels],
  );
  const meetingIds = React.useMemo(
    () => meetingRooms.map((channel) => channel.id).sort(),
    [meetingRooms],
  );
  const meetingDirectory = useMeetingDirectory(meetingIds);
  useMeetingLiveSync(meetingIds, meetingDirectory.data);
  const meetingItems = React.useMemo<MeetingListItem[]>(() => {
    const projected = new Map(
      (meetingDirectory.data ?? []).map((meeting) => [
        meeting.meetingId,
        meeting,
      ]),
    );
    return meetingRooms.map((room) => {
      const item = projected.get(room.id);
      if (item) {
        // Group metadata is the room discovery/title authority even when the
        // Meeting protocol itself must fail closed as unsupported.
        return { ...item, title: room.name };
      }
      return {
        meetingId: room.id,
        title: room.name,
        lifecycle: "initializing",
        phase: "initializing",
        currentSpeakerPubkey: null,
        currentOfferPubkey: null,
        needsAttention: false,
        attentionReason: null,
        moderatorPubkey: null,
        policy: null,
        updatedAt: room.lastMessageAt
          ? Math.floor(Date.parse(room.lastMessageAt) / 1_000)
          : null,
        endedAt: null,
        latestSpeechAt: null,
        compatibility: "ready",
      };
    });
  }, [meetingDirectory.data, meetingRooms]);
  const sidebarChannels = React.useMemo(
    () =>
      memberChannels.filter(
        (channel) =>
          channel.roomKind !== "meeting" && channel.archivedAt === null,
      ),
    [memberChannels],
  );

  return {
    conversationChannels,
    meetingItems,
    meetingRooms,
    sidebarChannels,
  };
}

/** Derive Meeting Speech unread state from the shared persistent read marker. */
export function useUnreadMeetingIds(
  meetingItems: MeetingListItem[],
  getChannelReadAt: (contextKey: string) => number | null,
  readStateVersion: number,
): ReadonlySet<string> {
  // biome-ignore lint/correctness/useExhaustiveDependencies: getChannelReadAt reads mutable manager state; readStateVersion is its explicit invalidation signal.
  return React.useMemo(() => {
    const unread = new Set<string>();
    for (const meeting of meetingItems) {
      if (!meeting.latestSpeechAt) continue;
      const readAt = getChannelReadAt(meeting.meetingId);
      if (readAt === null || meeting.latestSpeechAt > readAt) {
        unread.add(meeting.meetingId);
      }
    }
    return unread;
  }, [getChannelReadAt, meetingItems, readStateVersion]);
}
