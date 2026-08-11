import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { channelsQueryKey } from "@/features/channels/hooks";
import { useCommunities } from "@/features/communities/useCommunities";
import {
  PROJECT_CONTEXT_LIVE_LOOKBACK_SECONDS,
  ProjectContextInvalidationScheduler,
  projectContextInvalidationScopesForKind,
  projectContextLiveFilter,
  type ProjectContextInvalidationScope,
} from "@/features/project-context/liveSync";
import { relayClient } from "@/shared/api/relayClient";
import type { RelaySubscriptionFilter } from "@/shared/api/relayClientShared";
import {
  canonicalizeProjectContextQuery,
  projectContextQueryKey,
  queryProjectContext,
  type ProjectContextQuery,
  type ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";

export const ALL_PROJECT_CONTEXT_QUERY: ProjectContextQuery = {
  type: "contains_all",
  coordinates: [],
};

export function projectContextCommunityKey(input: {
  communityId?: string;
  reinitKey: number;
}): string {
  return `${input.communityId ?? "none"}-${input.reinitKey}`;
}

export function projectContextRelayOrigin(relayUrl?: string): string {
  if (!relayUrl) return "no-relay";
  try {
    const parsed = new URL(relayUrl);
    if (parsed.protocol === "ws:") parsed.protocol = "http:";
    if (parsed.protocol === "wss:") parsed.protocol = "https:";
    return parsed.origin;
  } catch {
    return relayUrl.trim().toLowerCase().replace(/\/+$/, "");
  }
}

export const projectContextCacheKey = (
  communityKey: string,
  relayOrigin: string,
  query: ProjectContextQuery,
) =>
  [
    "project-context",
    communityKey,
    relayOrigin,
    projectContextQueryKey(query),
  ] as const;

export function projectContextResultIdentity(
  result: ProjectContextQueryResult,
): string {
  return [
    result.projectId,
    result.relayPubkey.toLowerCase(),
    result.context.projectionGeneration,
  ].join(":");
}

export function isIncompatibleProjectContextCacheEntry(input: {
  queryKey: readonly unknown[];
  data: unknown;
  communityKey: string;
  identity: string;
}): boolean {
  if (
    input.queryKey[0] !== "project-context" ||
    input.queryKey[1] !== input.communityKey ||
    typeof input.data !== "object" ||
    input.data === null
  ) {
    return false;
  }
  const candidate = input.data as Partial<ProjectContextQueryResult>;
  if (
    typeof candidate.projectId !== "string" ||
    typeof candidate.relayPubkey !== "string" ||
    typeof candidate.context?.projectionGeneration !== "number"
  ) {
    return false;
  }
  return (
    projectContextResultIdentity(candidate as ProjectContextQueryResult) !==
    input.identity
  );
}

/** Read one canonical Project Context query within the active Community boundary. */
export function useProjectContextQuery(
  requestedQuery: ProjectContextQuery = ALL_PROJECT_CONTEXT_QUERY,
  options?: { enabled?: boolean },
) {
  const { activeCommunity, reinitKey } = useCommunities();
  const queryClient = useQueryClient();
  const query = canonicalizeProjectContextQuery(requestedQuery);
  const communityKey = projectContextCommunityKey({
    communityId: activeCommunity?.id,
    reinitKey,
  });
  const relayOrigin = projectContextRelayOrigin(activeCommunity?.relayUrl);
  const result = useQuery({
    queryKey: projectContextCacheKey(communityKey, relayOrigin, query),
    queryFn: () => queryProjectContext({ communityKey, query }),
    enabled: Boolean(activeCommunity) && options?.enabled !== false,
    retry: false,
    staleTime: 15_000,
    refetchOnWindowFocus: true,
  });

  React.useEffect(() => {
    if (!result.data) return;
    const identity = projectContextResultIdentity(result.data);
    queryClient.removeQueries({
      predicate: (candidate) =>
        isIncompatibleProjectContextCacheEntry({
          queryKey: candidate.queryKey,
          data: candidate.state.data,
          communityKey,
          identity,
        }),
    });
  }, [communityKey, queryClient, result.data]);

  return result;
}

export type ProjectContextLiveStatus =
  | "idle"
  | "connecting"
  | "live"
  | "retrying";

const LIVE_RETRY_BASE_MS = 1_000;
const LIVE_RETRY_MAX_MS = 30_000;
const NIP29_GROUP_METADATA_KIND = 39_000;
const SEMANTIC_MEETING_FILTER_BATCH_SIZE = 64;
const TRUSTED_REFRESH_OPTIONS = { throwOnError: true } as const;

function chunks<T>(items: readonly T[], size: number): T[][] {
  const result: T[][] = [];
  for (let index = 0; index < items.length; index += size) {
    result.push(items.slice(index, index + size));
  }
  return result;
}

/** Build bounded Relay-authored Meeting metadata hint filters for semantic sources. */
export function projectContextSemanticMeetingLiveFilters(input: {
  meetingIds: readonly string[];
  nowSeconds: number;
  relayPubkey: string;
}): RelaySubscriptionFilter[] {
  const meetingIds = [
    ...new Set(
      input.meetingIds.map((meetingId) => meetingId.trim()).filter(Boolean),
    ),
  ].sort();
  return chunks(meetingIds, SEMANTIC_MEETING_FILTER_BATCH_SIZE).map(
    (meetingIdBatch) => ({
      authors: [input.relayPubkey.trim().toLowerCase()],
      kinds: [NIP29_GROUP_METADATA_KIND],
      "#d": meetingIdBatch,
      limit: 256,
      since: Math.max(
        0,
        input.nowSeconds - PROJECT_CONTEXT_LIVE_LOOKBACK_SECONDS,
      ),
    }),
  );
}

/**
 * Treat Context, Project View, Document, and scoped Meeting projection events
 * only as hints. Every hint re-enters the relevant verified native boundary.
 */
export function useProjectContextLiveSync(
  snapshot?: ProjectContextQueryResult,
  semanticMeetingIds: readonly string[] = [],
): ProjectContextLiveStatus {
  const { activeCommunity, reinitKey } = useCommunities();
  const queryClient = useQueryClient();
  const [status, setStatus] = React.useState<ProjectContextLiveStatus>("idle");
  const communityId = activeCommunity?.id;
  const communityKey = projectContextCommunityKey({
    communityId,
    reinitKey,
  });
  const semanticMeetingIdsKey = [
    ...new Set(
      semanticMeetingIds.map((meetingId) => meetingId.trim()).filter(Boolean),
    ),
  ]
    .sort()
    .join(",");
  const stableSemanticMeetingIds = React.useMemo(
    () => (semanticMeetingIdsKey ? semanticMeetingIdsKey.split(",") : []),
    [semanticMeetingIdsKey],
  );
  const invalidate = React.useEffectEvent(
    async (scopes: ReadonlySet<ProjectContextInvalidationScope>) => {
      const refreshes: Array<Promise<unknown>> = [];
      if (scopes.has("context")) {
        refreshes.push(
          queryClient.invalidateQueries(
            {
              predicate: (candidate) =>
                candidate.queryKey[0] === "project-context" &&
                candidate.queryKey[1] === communityKey,
            },
            TRUSTED_REFRESH_OPTIONS,
          ),
        );
        if (stableSemanticMeetingIds.length > 0) {
          refreshes.push(
            queryClient.invalidateQueries(
              { queryKey: channelsQueryKey },
              TRUSTED_REFRESH_OPTIONS,
            ),
            queryClient.invalidateQueries(
              {
                predicate: (candidate) =>
                  candidate.queryKey[0] === "meetings" &&
                  candidate.queryKey[1] === (communityId ?? "no-community") &&
                  (candidate.queryKey[2] === "directory" ||
                    (candidate.queryKey[2] === "context-detail" &&
                      typeof candidate.queryKey[3] === "string" &&
                      stableSemanticMeetingIds.includes(
                        candidate.queryKey[3],
                      ))),
              },
              TRUSTED_REFRESH_OPTIONS,
            ),
          );
        }
      }
      if (scopes.has("project_view")) {
        refreshes.push(
          queryClient.invalidateQueries(
            {
              queryKey: ["project-view", communityId ?? "no-community"],
            },
            TRUSTED_REFRESH_OPTIONS,
          ),
        );
      }
      if (scopes.has("documents") || scopes.has("document_catalog")) {
        refreshes.push(
          queryClient.invalidateQueries(
            {
              predicate: (candidate) => {
                const root = candidate.queryKey[0];
                return (
                  (root === "project-document-meta" ||
                    root === "project-documents") &&
                  candidate.queryKey[1] === communityKey
                );
              },
            },
            TRUSTED_REFRESH_OPTIONS,
          ),
        );
      }
      if (scopes.has("documents")) {
        refreshes.push(
          queryClient.invalidateQueries(
            {
              predicate: (candidate) =>
                (candidate.queryKey[0] === "project-document-history" ||
                  (candidate.queryKey[0] === "project-document" &&
                    candidate.queryKey[6] === "current")) &&
                candidate.queryKey[1] === communityKey,
            },
            TRUSTED_REFRESH_OPTIONS,
          ),
        );
      }
      await Promise.all(refreshes);
    },
  );

  const relayPubkey = snapshot?.relayPubkey.trim().toLowerCase();
  const contextUpdatedAt = snapshot?.context.updatedAt;
  const projectViewUpdatedAt = snapshot?.projectViewObservation.updatedAt;
  const documentUpdatedAt = snapshot?.documentObservation.updatedAt;
  const boundaryIsCurrent = snapshot?.communityKey === communityKey;

  React.useEffect(() => {
    if (
      !communityId ||
      !boundaryIsCurrent ||
      !relayPubkey ||
      !contextUpdatedAt
    ) {
      setStatus("idle");
      return;
    }

    let cancelled = false;
    let retryAttempt = 0;
    let retryTimer: number | null = null;
    let catchupAttempt = 0;
    let catchupGeneration = 0;
    let catchupRetryTimer: number | null = null;
    let disposeSubscriptions: Array<() => Promise<void>> = [];
    const semanticMeetingIdSet = new Set(stableSemanticMeetingIds);
    const completeRefreshScopes = new Set<ProjectContextInvalidationScope>([
      "context",
      "project_view",
      "document_catalog",
    ]);
    const scheduler = new ProjectContextInvalidationScheduler(
      (scopes) => (cancelled ? undefined : invalidate(scopes)),
      undefined,
      window.setTimeout.bind(window),
      window.clearTimeout.bind(window),
    );

    const catchUpAfterReconnect = async () => {
      if (cancelled) return;
      const generation = ++catchupGeneration;
      setStatus(catchupAttempt === 0 ? "connecting" : "retrying");
      try {
        await invalidate(completeRefreshScopes);
        if (cancelled || generation !== catchupGeneration) return;
        catchupAttempt = 0;
        setStatus("live");
      } catch {
        if (cancelled || generation !== catchupGeneration) return;
        setStatus("retrying");
        const delay = Math.min(
          LIVE_RETRY_MAX_MS,
          LIVE_RETRY_BASE_MS * 2 ** Math.min(catchupAttempt, 5),
        );
        catchupAttempt += 1;
        catchupRetryTimer = window.setTimeout(() => {
          catchupRetryTimer = null;
          void catchUpAfterReconnect();
        }, delay);
      }
    };
    const removeReconnectListener = relayClient.subscribeToReconnects(() => {
      catchupAttempt = 0;
      if (catchupRetryTimer !== null) {
        window.clearTimeout(catchupRetryTimer);
        catchupRetryTimer = null;
      }
      void catchUpAfterReconnect();
    });

    const subscribe = async () => {
      if (cancelled) return;
      setStatus(retryAttempt === 0 ? "connecting" : "retrying");
      const attemptDisposers: Array<() => Promise<void>> = [];
      try {
        const dispose = await relayClient.subscribeLive(
          projectContextLiveFilter({
            relayPubkey,
            contextUpdatedAt,
            projectViewUpdatedAt,
            documentUpdatedAt,
          }),
          (event) => {
            if (cancelled) return;
            const scopes = projectContextInvalidationScopesForKind(event.kind);
            if (scopes.length > 0) scheduler.signal(scopes);
          },
        );
        attemptDisposers.push(dispose);
        const subscriptionStart = Math.floor(Date.now() / 1_000);
        for (const meetingFilter of projectContextSemanticMeetingLiveFilters({
          meetingIds: stableSemanticMeetingIds,
          nowSeconds: subscriptionStart,
          relayPubkey,
        })) {
          const disposeMeetingMetadata = await relayClient.subscribeLive(
            meetingFilter,
            (event) => {
              const meetingId = event.tags.find((tag) => tag[0] === "d")?.[1];
              if (
                !cancelled &&
                meetingId !== undefined &&
                semanticMeetingIdSet.has(meetingId)
              ) {
                scheduler.signal("context");
              }
            },
          );
          attemptDisposers.push(disposeMeetingMetadata);
        }
        if (cancelled) {
          for (const disposeAttempt of attemptDisposers) {
            void disposeAttempt().catch(() => {});
          }
          return;
        }
        disposeSubscriptions = attemptDisposers;

        // Close every observation-to-subscription race before advertising the
        // screen as Live. Future projection hints still use the coalescing
        // scheduler, but this first catch-up must be awaitable so it cannot
        // arrive after an unrelated route-only selection change.
        try {
          await invalidate(completeRefreshScopes);
        } catch (error) {
          disposeSubscriptions = [];
          await Promise.all(
            attemptDisposers.map((disposeAttempt) =>
              disposeAttempt().catch(() => {}),
            ),
          );
          throw error;
        }
        if (cancelled) return;
        retryAttempt = 0;
        setStatus("live");
      } catch (error) {
        if (disposeSubscriptions.length === 0) {
          await Promise.all(
            attemptDisposers.map((disposeAttempt) =>
              disposeAttempt().catch(() => {}),
            ),
          );
        }
        if (cancelled) return;
        console.error(
          "Failed to subscribe to Project Context projection updates; retrying",
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
      catchupGeneration += 1;
      scheduler.dispose();
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (catchupRetryTimer !== null) {
        window.clearTimeout(catchupRetryTimer);
      }
      removeReconnectListener();
      for (const disposeSubscription of disposeSubscriptions) {
        void disposeSubscription().catch(() => {});
      }
    };
  }, [
    boundaryIsCurrent,
    communityId,
    contextUpdatedAt,
    documentUpdatedAt,
    projectViewUpdatedAt,
    relayPubkey,
    stableSemanticMeetingIds,
  ]);

  return status;
}
