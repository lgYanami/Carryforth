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
  getMeetingContextDetail,
  getMeetingSnapshot,
  getMeetingSpeeches,
  listMeetings,
  submitMeetingActionFinalization,
  submitMeetingFloorAction,
  submitMeetingHostAction,
  type MeetingActionFinalizationInput,
  type MeetingFloorActionInput,
  type MeetingHostActionInput,
  type MeetingListItem,
  type MeetingSpeechCursor,
} from "@/shared/api/tauriMeetings";
import type { Channel } from "@/shared/api/types";
import {
  KIND_MEETING_END,
  KIND_MEETING_STATE,
  KIND_STREAM_MESSAGE,
} from "@/shared/constants/kinds";
import {
  isTerminalMeetingLifecycle,
  meetingDirectoryFallbackInterval,
  meetingLiveSubscriptionIds,
  meetingSnapshotFallbackInterval,
} from "./meetingSyncPolicy";
import {
  MeetingLiveInvalidationScheduler,
  type MeetingLiveSignal,
  MeetingLiveSubscriptionManager,
} from "./liveSync";

const MEETING_DIRECTORY_BATCH_SIZE = 64;

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

export const meetingContextDetailQueryKey = (
  communityId: string | undefined,
  meetingId: string,
) => [...meetingQueryRoot(communityId), "context-detail", meetingId] as const;

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
    refetchInterval: (query) =>
      meetingDirectoryFallbackInterval(query.state.data),
    refetchIntervalInBackground: false,
  });
}

export function useMeetingSnapshot(meetingId: string) {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: meetingSnapshotQueryKey(activeCommunity?.id, meetingId),
    queryFn: () => getMeetingSnapshot(meetingId),
    enabled: Boolean(activeCommunity && meetingId),
    staleTime: 5_000,
    refetchOnMount: "always",
    refetchOnWindowFocus: true,
    refetchInterval: (snapshotQuery) =>
      meetingSnapshotFallbackInterval(snapshotQuery.state.data),
    refetchIntervalInBackground: false,
  });
  const terminalSnapshot =
    query.data?.status === "ready" &&
    isTerminalMeetingLifecycle(query.data.snapshot.lifecycle)
      ? query.data.snapshot
      : null;
  const terminalRevisionKey = terminalSnapshot
    ? `${activeCommunity?.id ?? "no-community"}:${meetingId}:${terminalSnapshot.lifecycle}:${terminalSnapshot.stateRevision}`
    : null;
  const lastReconciledTerminal = React.useRef<string | null>(null);

  React.useEffect(() => {
    if (!terminalRevisionKey) {
      lastReconciledTerminal.current = null;
      return;
    }
    if (lastReconciledTerminal.current === terminalRevisionKey) return;
    lastReconciledTerminal.current = terminalRevisionKey;
    void queryClient.invalidateQueries({
      queryKey: [...meetingQueryRoot(activeCommunity?.id), "directory"],
      refetchType: "active",
    });
  }, [activeCommunity?.id, queryClient, terminalRevisionKey]);

  return query;
}

/** Read body-free terminal metadata for Project Context inspection. */
export function useMeetingContextDetail(meetingId: string) {
  const { activeCommunity } = useCommunities();
  return useQuery({
    queryKey: meetingContextDetailQueryKey(activeCommunity?.id, meetingId),
    queryFn: () => getMeetingContextDetail(meetingId),
    enabled: Boolean(activeCommunity && meetingId),
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
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
export function useMeetingLiveSync(
  meetingIds: readonly string[],
  meetings: readonly MeetingListItem[] | undefined,
): void {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();
  const communityId = activeCommunity?.id;
  const meetingIdsKey = meetingLiveSubscriptionIds(meetingIds, meetings).join(
    ",",
  );
  const stableIds = React.useMemo(
    () => (meetingIdsKey ? meetingIdsKey.split(",") : []),
    [meetingIdsKey],
  );
  const invalidate = React.useEffectEvent(
    async (meetingId: string, signals: ReadonlySet<MeetingLiveSignal>) => {
      const invalidations: Promise<unknown>[] = [
        queryClient.invalidateQueries({
          queryKey: meetingSnapshotQueryKey(communityId, meetingId),
        }),
        queryClient.invalidateQueries({
          queryKey: [...meetingQueryRoot(communityId), "directory"],
        }),
      ];
      if (signals.has(KIND_STREAM_MESSAGE)) {
        invalidations.push(
          queryClient.invalidateQueries({
            queryKey: meetingSpeechesQueryKey(communityId, meetingId),
          }),
        );
      }
      if (signals.has(KIND_MEETING_STATE) || signals.has(KIND_MEETING_END)) {
        invalidations.push(
          queryClient.invalidateQueries({
            queryKey: meetingActivitiesQueryKey(communityId, meetingId),
          }),
        );
      }
      if (signals.has(KIND_MEETING_END)) {
        invalidations.push(
          queryClient.invalidateQueries({ queryKey: channelsQueryKey }),
        );
      }
      await Promise.all(invalidations);
    },
  );
  const managerRef = React.useRef<MeetingLiveSubscriptionManager | null>(null);

  React.useEffect(() => {
    if (!communityId) {
      managerRef.current = null;
      return;
    }

    const schedulers = new Map<string, MeetingLiveInvalidationScheduler>();
    const schedulerFor = (meetingId: string) => {
      const existing = schedulers.get(meetingId);
      if (existing) return existing;
      const scheduler = new MeetingLiveInvalidationScheduler(
        (signals) => invalidate(meetingId, signals),
        undefined,
        window.setTimeout.bind(window),
        window.clearTimeout.bind(window),
      );
      schedulers.set(meetingId, scheduler);
      return scheduler;
    };
    const manager = new MeetingLiveSubscriptionManager({
      subscribe: (filter, onEvent) =>
        relayClient.subscribeLive(filter, onEvent),
      onSignal: (meetingId, signal) => schedulerFor(meetingId).signal(signal),
      onError: (meetingId, error, retryInMs) => {
        console.error("Failed to subscribe to Meeting updates; retrying", {
          meetingId,
          retryInMs,
          error,
        });
      },
      setTimeoutFn: window.setTimeout.bind(window),
      clearTimeoutFn: window.clearTimeout.bind(window),
    });
    managerRef.current = manager;

    return () => {
      if (managerRef.current === manager) managerRef.current = null;
      manager.destroy();
      for (const scheduler of schedulers.values()) scheduler.dispose();
      schedulers.clear();
    };
  }, [communityId]);

  React.useEffect(() => {
    if (!communityId) return;
    managerRef.current?.sync(stableIds);
  }, [communityId, stableIds]);
}
