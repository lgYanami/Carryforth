import * as React from "react";
import {
  useInfiniteQuery,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import { useCommunities } from "@/features/communities/useCommunities";
import {
  channelsQueryKey,
  upsertCachedChannel,
} from "@/features/channels/hooks";
import { relayClient } from "@/shared/api/relayClient";
import {
  createMeeting,
  getMeetingActivities,
  getMeetingCapability,
  getMeetingSnapshot,
  getMeetingSpeeches,
  listMeetings,
  submitMeetingActionFinalization,
  submitMeetingFloorAction,
  submitMeetingHostAction,
  type MeetingActionFinalizationInput,
  type MeetingFloorActionInput,
  type MeetingHostActionInput,
  type MeetingSpeechCursor,
} from "@/shared/api/tauriMeetings";
import type { Channel } from "@/shared/api/types";
import {
  KIND_MEETING_END,
  KIND_MEETING_STATE,
  KIND_STREAM_MESSAGE,
} from "@/shared/constants/kinds";

const MEETING_LIVE_LOOKBACK_SECONDS = 5;
const MEETING_INVALIDATION_DELAY_MS = 150;
const MEETING_SUBSCRIPTION_RETRY_BASE_MS = 500;
const MEETING_SUBSCRIPTION_RETRY_MAX_MS = 5_000;
const MEETING_DIRECTORY_BATCH_SIZE = 64;
const MEETING_LIVE_BATCH_SIZE = 64;

function chunks<T>(items: readonly T[], size: number): T[][] {
  const result: T[][] = [];
  for (let index = 0; index < items.length; index += size) {
    result.push(items.slice(index, index + size));
  }
  return result;
}

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

export const meetingActivitiesQueryKey = (
  communityId: string | undefined,
  meetingId: string,
) => [...meetingQueryRoot(communityId), "activities", meetingId] as const;

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

export function useCreateMeetingMutation() {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: createMeeting,
    onSuccess: (result, input) => {
      if (result.status !== "accepted") return;

      const participantPubkeys = [
        result.hostPubkey,
        ...result.participantPubkeys,
      ];
      const acceptedRoom: Channel = {
        id: result.meetingId,
        name: result.title,
        channelType: "stream",
        roomKind: "meeting",
        visibility: "private",
        description: input.description ?? "",
        topic: null,
        purpose: null,
        memberCount: participantPubkeys.length,
        memberPubkeys: participantPubkeys,
        lastMessageAt: null,
        archivedAt: null,
        participants: [],
        participantPubkeys: [],
        isMember: true,
        ttlSeconds: null,
        ttlDeadline: null,
      };
      queryClient.setQueryData<Channel[]>(channelsQueryKey, (current) =>
        upsertCachedChannel(current, acceptedRoom),
      );
      void queryClient.invalidateQueries({
        queryKey: meetingQueryRoot(activeCommunity?.id),
        refetchType: "none",
      });
      void queryClient.invalidateQueries({
        queryKey: channelsQueryKey,
        refetchType: "none",
      });
    },
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
    queryFn: async () => {
      const meetings = [];
      for (const batch of chunks(stableIds, MEETING_DIRECTORY_BATCH_SIZE)) {
        meetings.push(...(await listMeetings(batch)));
      }
      return meetings;
    },
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
    refetchOnMount: "always",
    refetchOnWindowFocus: true,
  });
}

export function useMeetingFloorActionMutation(meetingId: string) {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (input: MeetingFloorActionInput) =>
      submitMeetingFloorAction(input),
    onSuccess: async (result) => {
      if (result.status !== "accepted") return;
      const invalidations: Promise<unknown>[] = [
        queryClient.invalidateQueries({
          queryKey: meetingSnapshotQueryKey(activeCommunity?.id, meetingId),
        }),
        queryClient.invalidateQueries({
          queryKey: [...meetingQueryRoot(activeCommunity?.id), "directory"],
        }),
      ];
      if (result.action === "speech") {
        invalidations.push(
          queryClient.invalidateQueries({
            queryKey: meetingSpeechesQueryKey(activeCommunity?.id, meetingId),
          }),
        );
      }
      await Promise.all(invalidations);
    },
    onError: async () => {
      await queryClient.invalidateQueries({
        queryKey: meetingSnapshotQueryKey(activeCommunity?.id, meetingId),
      });
    },
  });
}

export function useMeetingHostActionMutation(meetingId: string) {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (input: MeetingHostActionInput) =>
      submitMeetingHostAction(input),
    onSuccess: async (result) => {
      if (result.status !== "accepted") return;
      const invalidations: Promise<unknown>[] = [
        queryClient.invalidateQueries({
          queryKey: meetingSnapshotQueryKey(activeCommunity?.id, meetingId),
        }),
        queryClient.invalidateQueries({
          queryKey: [...meetingQueryRoot(activeCommunity?.id), "directory"],
        }),
      ];
      if (result.action === "close" || result.action === "abort") {
        invalidations.push(
          queryClient.invalidateQueries({ queryKey: channelsQueryKey }),
        );
      }
      await Promise.all(invalidations);
    },
    onError: async () => {
      await queryClient.invalidateQueries({
        queryKey: meetingSnapshotQueryKey(activeCommunity?.id, meetingId),
      });
    },
  });
}

export function useMeetingActionFinalizationMutation(meetingId: string) {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (input: MeetingActionFinalizationInput) =>
      submitMeetingActionFinalization(input),
    onSuccess: async (result) => {
      if (result.status !== "accepted") return;
      const invalidations: Promise<unknown>[] = [
        queryClient.invalidateQueries({
          queryKey: meetingSnapshotQueryKey(activeCommunity?.id, meetingId),
        }),
        queryClient.invalidateQueries({
          queryKey: [...meetingQueryRoot(activeCommunity?.id), "directory"],
        }),
      ];
      if (result.action === "confirm") {
        invalidations.push(
          queryClient.invalidateQueries({ queryKey: channelsQueryKey }),
        );
      }
      await Promise.all(invalidations);
    },
    onError: async () => {
      await queryClient.invalidateQueries({
        queryKey: meetingSnapshotQueryKey(activeCommunity?.id, meetingId),
      });
    },
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

export function useMeetingActivities(input: {
  meetingId: string;
  enabled: boolean;
}) {
  const { activeCommunity } = useCommunities();
  return useInfiniteQuery({
    queryKey: meetingActivitiesQueryKey(activeCommunity?.id, input.meetingId),
    queryFn: ({ pageParam }) =>
      getMeetingActivities({
        meetingId: input.meetingId,
        cursor: pageParam,
      }),
    initialPageParam: undefined as string | undefined,
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
    let disposeSubscriptions: Array<() => Promise<void>> = [];
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
      const nextSubscriptions: Array<() => Promise<void>> = [];
      try {
        for (const batch of chunks(stableIds, MEETING_LIVE_BATCH_SIZE)) {
          nextSubscriptions.push(
            await relayClient.subscribeLive(
              {
                kinds: [
                  KIND_STREAM_MESSAGE,
                  KIND_MEETING_STATE,
                  KIND_MEETING_END,
                ],
                "#h": batch,
                limit: 256,
                since: Math.max(
                  0,
                  Math.floor(Date.now() / 1_000) -
                    MEETING_LIVE_LOOKBACK_SECONDS,
                ),
              },
              signal,
            ),
          );
        }
        if (cancelled) {
          for (const dispose of nextSubscriptions) {
            void dispose().catch(() => {});
          }
          return;
        }
        disposeSubscriptions = nextSubscriptions;
        retryAttempt = 0;
        // Close the snapshot → subscription race.
        signal();
      } catch (error) {
        for (const dispose of nextSubscriptions) {
          void dispose().catch(() => {});
        }
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
      for (const dispose of disposeSubscriptions) {
        void dispose().catch(() => {});
      }
    };
  }, [communityId, stableIds]);
}
