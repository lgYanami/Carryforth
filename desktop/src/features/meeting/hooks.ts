import * as React from "react";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { useCommunities } from "@/features/communities/useCommunities";
import { relayClient } from "@/shared/api/relayClient";
import {
  getMeetingCapability,
  getMeetingSnapshot,
  getMeetingSpeeches,
  listMeetings,
  type MeetingSpeechCursor,
} from "@/shared/api/tauriMeetings";
import {
  KIND_MEETING_END,
  KIND_MEETING_STATE,
  KIND_STREAM_MESSAGE,
} from "@/shared/constants/kinds";

const MEETING_LIVE_LOOKBACK_SECONDS = 5;
const MEETING_INVALIDATION_DELAY_MS = 150;
const MEETING_SUBSCRIPTION_RETRY_BASE_MS = 500;
const MEETING_SUBSCRIPTION_RETRY_MAX_MS = 5_000;

export const meetingQueryRoot = (communityId: string | undefined) =>
  ["meetings", communityId ?? "no-community"] as const;

export const meetingCapabilityQueryKey = (communityId: string | undefined) =>
  [...meetingQueryRoot(communityId), "capability"] as const;

export const meetingDirectoryQueryKey = (
  communityId: string | undefined,
  meetingIds: readonly string[],
) => [...meetingQueryRoot(communityId), "directory", ...meetingIds] as const;

export const meetingSnapshotQueryKey = (
  communityId: string | undefined,
  meetingId: string,
) => [...meetingQueryRoot(communityId), "snapshot", meetingId] as const;

export const meetingSpeechesQueryKey = (
  communityId: string | undefined,
  meetingId: string,
) => [...meetingQueryRoot(communityId), "speeches", meetingId] as const;

export function useMeetingCapability() {
  const { activeCommunity } = useCommunities();
  return useQuery({
    queryKey: meetingCapabilityQueryKey(activeCommunity?.id),
    queryFn: getMeetingCapability,
    enabled: Boolean(activeCommunity),
    staleTime: 30_000,
    refetchOnWindowFocus: true,
  });
}

export function useMeetingDirectory(meetingIds: readonly string[]) {
  const { activeCommunity } = useCommunities();
  const meetingIdsKey = [...new Set(meetingIds)].sort().join(",");
  const stableIds = React.useMemo(
    () => (meetingIdsKey ? meetingIdsKey.split(",") : []),
    [meetingIdsKey],
  );
  return useQuery({
    queryKey: meetingDirectoryQueryKey(activeCommunity?.id, stableIds),
    queryFn: () => listMeetings(stableIds),
    enabled: Boolean(activeCommunity) && stableIds.length > 0,
    staleTime: 10_000,
    refetchOnWindowFocus: true,
  });
}

export function useMeetingSnapshot(meetingId: string) {
  const { activeCommunity } = useCommunities();
  return useQuery({
    queryKey: meetingSnapshotQueryKey(activeCommunity?.id, meetingId),
    queryFn: () => getMeetingSnapshot(meetingId),
    enabled: Boolean(activeCommunity && meetingId),
    staleTime: 5_000,
    refetchOnWindowFocus: true,
  });
}

export function useMeetingSpeeches(input: {
  meetingId: string;
  enabled: boolean;
}) {
  const { activeCommunity } = useCommunities();
  return useInfiniteQuery({
    queryKey: meetingSpeechesQueryKey(activeCommunity?.id, input.meetingId),
    queryFn: ({ pageParam }) =>
      getMeetingSpeeches({
        meetingId: input.meetingId,
        before: pageParam,
      }),
    initialPageParam: undefined as MeetingSpeechCursor | undefined,
    getNextPageParam: (page) => page.nextCursor ?? undefined,
    enabled: Boolean(activeCommunity && input.meetingId && input.enabled),
    staleTime: 5_000,
  });
}

/**
 * Treat Meeting live events only as invalidation signals. The native bridge
 * re-reads and verifies the authoritative projection before React changes.
 */
export function useMeetingLiveSync(meetingIds: readonly string[]): void {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();
  const communityId = activeCommunity?.id;
  const meetingIdsKey = [...new Set(meetingIds)].sort().join(",");
  const stableIds = React.useMemo(
    () => (meetingIdsKey ? meetingIdsKey.split(",") : []),
    [meetingIdsKey],
  );
  const invalidate = React.useEffectEvent(async () => {
    await queryClient.invalidateQueries({
      queryKey: meetingQueryRoot(communityId),
    });
  });

  React.useEffect(() => {
    if (!communityId || stableIds.length === 0) return;

    let cancelled = false;
    let timer: number | null = null;
    let retryTimer: number | null = null;
    let disposeSubscription: (() => Promise<void>) | undefined;
    const signal = () => {
      if (cancelled) return;
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        timer = null;
        void invalidate();
      }, MEETING_INVALIDATION_DELAY_MS);
    };

    let retryAttempt = 0;
    const subscribe = async () => {
      try {
        const dispose = await relayClient.subscribeLive(
          {
            kinds: [KIND_STREAM_MESSAGE, KIND_MEETING_STATE, KIND_MEETING_END],
            "#h": stableIds,
            limit: 256,
            since: Math.max(
              0,
              Math.floor(Date.now() / 1_000) - MEETING_LIVE_LOOKBACK_SECONDS,
            ),
          },
          signal,
        );
        if (cancelled) {
          void dispose().catch(() => {});
          return;
        }
        disposeSubscription = dispose;
        retryAttempt = 0;
        // Close the snapshot → subscription race.
        signal();
      } catch (error) {
        if (cancelled) return;
        console.error("Failed to subscribe to Meeting updates", error);
        const delay = Math.min(
          MEETING_SUBSCRIPTION_RETRY_MAX_MS,
          MEETING_SUBSCRIPTION_RETRY_BASE_MS * 2 ** retryAttempt,
        );
        retryAttempt += 1;
        retryTimer = window.setTimeout(() => {
          retryTimer = null;
          void subscribe();
        }, delay);
      }
    };
    void subscribe();

    return () => {
      cancelled = true;
      if (timer !== null) window.clearTimeout(timer);
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (disposeSubscription) void disposeSubscription().catch(() => {});
    };
  }, [communityId, stableIds]);
}
