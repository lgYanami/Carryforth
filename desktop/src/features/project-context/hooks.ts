import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { useCommunities } from "@/features/communities/useCommunities";
import {
  ProjectContextInvalidationScheduler,
  projectContextInvalidationScopesForKind,
  projectContextLiveFilter,
  type ProjectContextInvalidationScope,
} from "@/features/project-context/liveSync";
import { relayClient } from "@/shared/api/relayClient";
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
    enabled: Boolean(activeCommunity),
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

/**
 * Treat Context, Project View, and Document projection events only as hints.
 * Every hint re-enters the relevant verified native read boundary.
 */
export function useProjectContextLiveSync(
  snapshot?: ProjectContextQueryResult,
): ProjectContextLiveStatus {
  const { activeCommunity, reinitKey } = useCommunities();
  const queryClient = useQueryClient();
  const [status, setStatus] = React.useState<ProjectContextLiveStatus>("idle");
  const communityId = activeCommunity?.id;
  const communityKey = projectContextCommunityKey({
    communityId,
    reinitKey,
  });
  const invalidate = React.useEffectEvent(
    async (scopes: ReadonlySet<ProjectContextInvalidationScope>) => {
      const refreshes: Array<Promise<unknown>> = [];
      if (scopes.has("context")) {
        refreshes.push(
          queryClient.invalidateQueries({
            predicate: (candidate) =>
              candidate.queryKey[0] === "project-context" &&
              candidate.queryKey[1] === communityKey,
          }),
        );
      }
      if (scopes.has("project_view")) {
        refreshes.push(
          queryClient.invalidateQueries({
            queryKey: ["project-view", communityId ?? "no-community"],
          }),
        );
      }
      if (scopes.has("documents") || scopes.has("document_catalog")) {
        refreshes.push(
          queryClient.invalidateQueries({
            predicate: (candidate) => {
              const root = candidate.queryKey[0];
              return (
                (root === "project-document-meta" ||
                  root === "project-documents") &&
                candidate.queryKey[1] === communityKey
              );
            },
          }),
        );
      }
      if (scopes.has("documents")) {
        refreshes.push(
          queryClient.invalidateQueries({
            predicate: (candidate) =>
              (candidate.queryKey[0] === "project-document-history" ||
                (candidate.queryKey[0] === "project-document" &&
                  candidate.queryKey[6] === "current")) &&
              candidate.queryKey[1] === communityKey,
          }),
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
    let disposeSubscription: (() => Promise<void>) | undefined;
    const scheduler = new ProjectContextInvalidationScheduler(
      (scopes) => (cancelled ? undefined : invalidate(scopes)),
      undefined,
      window.setTimeout.bind(window),
      window.clearTimeout.bind(window),
    );

    const subscribe = async () => {
      if (cancelled) return;
      setStatus(retryAttempt === 0 ? "connecting" : "retrying");
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
        if (cancelled) {
          void dispose().catch(() => {});
          return;
        }
        disposeSubscription = dispose;

        // Close every observation-to-subscription race before advertising the
        // screen as Live. Future projection hints still use the coalescing
        // scheduler, but this first catch-up must be awaitable so it cannot
        // arrive after an unrelated route-only selection change.
        try {
          await invalidate(
            new Set(["context", "project_view", "document_catalog"]),
          );
        } catch (error) {
          disposeSubscription = undefined;
          await dispose().catch(() => {});
          throw error;
        }
        if (cancelled) return;
        retryAttempt = 0;
        setStatus("live");
      } catch (error) {
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
      scheduler.dispose();
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (disposeSubscription) void disposeSubscription().catch(() => {});
    };
  }, [
    boundaryIsCurrent,
    communityId,
    contextUpdatedAt,
    documentUpdatedAt,
    projectViewUpdatedAt,
    relayPubkey,
  ]);

  return status;
}
