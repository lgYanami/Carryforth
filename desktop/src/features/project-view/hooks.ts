import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useCommunities } from "@/features/communities/useCommunities";
import {
  projectViewLiveFilter,
  ProjectViewInvalidationScheduler,
} from "@/features/project-view/liveSync";
import { relayClient } from "@/shared/api/relayClient";
import {
  getProjectView,
  mutateProjectView,
} from "@/shared/api/tauriProjectView";

export const projectViewQueryKey = (communityId: string | undefined) =>
  ["project-view", communityId ?? "no-community"] as const;

export function useProjectViewQuery() {
  const { activeCommunity } = useCommunities();
  return useQuery({
    queryKey: projectViewQueryKey(activeCommunity?.id),
    queryFn: getProjectView,
    enabled: Boolean(activeCommunity),
    staleTime: 15_000,
    refetchOnWindowFocus: true,
  });
}

export function useProjectViewMutation() {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();
  const communityId = activeCommunity?.id;

  return useMutation({
    mutationFn: mutateProjectView,
    onSuccess: (result) => {
      if (result.status !== "applied") return undefined;
      return queryClient.invalidateQueries({
        queryKey: projectViewQueryKey(communityId),
      });
    },
  });
}

export type ProjectViewLiveStatus = "idle" | "connecting" | "live" | "retrying";

const LIVE_RETRY_BASE_MS = 1_000;
const LIVE_RETRY_MAX_MS = 30_000;

/**
 * Watches Relay-authored Project View projection events as invalidation
 * signals. Event payloads never enter UI state: each signal causes the native
 * boundary to verify and assemble another complete snapshot.
 */
export function useProjectViewLiveSync(input: {
  relayPubkey?: string;
  snapshotUpdatedAt?: string;
}): ProjectViewLiveStatus {
  const { activeCommunity } = useCommunities();
  const queryClient = useQueryClient();
  const communityId = activeCommunity?.id;
  const [status, setStatus] = React.useState<ProjectViewLiveStatus>("idle");
  const invalidate = React.useEffectEvent(async () => {
    await queryClient.invalidateQueries({
      queryKey: projectViewQueryKey(communityId),
    });
  });

  React.useEffect(() => {
    const relayPubkey = input.relayPubkey?.trim().toLowerCase();
    if (!communityId || !relayPubkey) {
      setStatus("idle");
      return;
    }

    let cancelled = false;
    let retryAttempt = 0;
    let retryTimer: number | null = null;
    let disposeSubscription: (() => Promise<void>) | undefined;
    const scheduler = new ProjectViewInvalidationScheduler(
      async () => {
        if (!cancelled) await invalidate();
      },
      undefined,
      window.setTimeout.bind(window),
      window.clearTimeout.bind(window),
    );

    const subscribe = async () => {
      if (cancelled) return;
      setStatus(retryAttempt === 0 ? "connecting" : "retrying");
      try {
        const dispose = await relayClient.subscribeLive(
          projectViewLiveFilter({
            relayPubkey,
            snapshotUpdatedAt: input.snapshotUpdatedAt,
          }),
          () => {
            if (!cancelled) scheduler.signal();
          },
        );
        if (cancelled) {
          void dispose().catch(() => {});
          return;
        }
        disposeSubscription = dispose;
        retryAttempt = 0;
        setStatus("live");

        // Close the snapshot→subscription race. This is also the explicit
        // re-confirmation when the View mounts after a Community switch.
        scheduler.signal();
      } catch (error) {
        if (cancelled) return;
        console.error(
          "Failed to subscribe to Project View projection updates; retrying",
          error,
        );
        setStatus("retrying");
        const delay = Math.min(
          LIVE_RETRY_MAX_MS,
          LIVE_RETRY_BASE_MS * 2 ** Math.min(retryAttempt, 5),
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
      scheduler.dispose();
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (disposeSubscription) {
        void disposeSubscription().catch(() => {});
      }
    };
  }, [communityId, input.relayPubkey, input.snapshotUpdatedAt]);

  return status;
}
