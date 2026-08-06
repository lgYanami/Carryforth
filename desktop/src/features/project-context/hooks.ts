import * as React from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";

import { useCommunities } from "@/features/communities/useCommunities";
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
