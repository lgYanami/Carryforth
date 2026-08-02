import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useCommunities } from "@/features/communities/useCommunities";
import {
  ProjectDocumentInvalidationScheduler,
  projectDocumentLiveFilter,
} from "@/features/project-documents/liveSync";
import { relayClient } from "@/shared/api/relayClient";
import {
  documentIdentity,
  getProjectDocument,
  getProjectDocumentHistory,
  getProjectDocumentMeta,
  listProjectDocuments,
  mutateProjectDocument,
  type ProjectDocumentIdentity,
  type ProjectDocumentMeta,
  type ProjectDocumentMutation,
} from "@/shared/api/tauriProjectDocument";

export function projectDocumentCommunityKey(input: {
  communityId?: string;
  reinitKey: number;
}): string {
  return `${input.communityId ?? "none"}-${input.reinitKey}`;
}

export function projectDocumentRelayOrigin(relayUrl?: string): string {
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

export const projectDocumentMetaKey = (
  communityKey: string,
  relayOrigin: string,
) => ["project-document-meta", communityKey, relayOrigin] as const;

export const projectDocumentsKey = (meta: ProjectDocumentMeta) =>
  [
    "project-documents",
    meta.communityKey,
    meta.projectId,
    meta.relayPubkey,
    meta.projectionGeneration,
    meta.catalogRevision,
  ] as const;

export const projectDocumentKey = (
  identity: ProjectDocumentIdentity,
  documentId: string,
  revision: number | "current",
) =>
  [
    "project-document",
    identity.communityKey,
    identity.projectId,
    identity.relayPubkey,
    identity.projectionGeneration,
    documentId,
    revision,
  ] as const;

export const projectDocumentHistoryKey = (
  identity: ProjectDocumentIdentity,
  documentId: string,
  maxDocumentRevision: number,
) =>
  [
    "project-document-history",
    identity.communityKey,
    identity.projectId,
    identity.relayPubkey,
    identity.projectionGeneration,
    documentId,
    maxDocumentRevision,
  ] as const;

function isDocumentBusinessKeyForCommunity(
  queryKey: readonly unknown[],
  communityKey: string,
): boolean {
  return (
    typeof queryKey[0] === "string" &&
    queryKey[0] !== "project-document-meta" &&
    queryKey[0].startsWith("project-document") &&
    queryKey[1] === communityKey
  );
}

function useDocumentCommunity() {
  const { activeCommunity, reinitKey } = useCommunities();
  const communityKey = projectDocumentCommunityKey({
    communityId: activeCommunity?.id,
    reinitKey,
  });
  return {
    activeCommunity,
    communityKey,
    relayOrigin: projectDocumentRelayOrigin(activeCommunity?.relayUrl),
  };
}

export function useProjectDocumentMeta(enabled = true) {
  const queryClient = useQueryClient();
  const { activeCommunity, communityKey, relayOrigin } = useDocumentCommunity();
  const previousIdentityRef = React.useRef<string | null>(null);
  const query = useQuery({
    queryKey: projectDocumentMetaKey(communityKey, relayOrigin),
    queryFn: () => getProjectDocumentMeta(communityKey),
    enabled: Boolean(activeCommunity) && enabled,
    staleTime: 15_000,
    refetchOnWindowFocus: true,
  });

  React.useEffect(() => {
    if (!query.data) return;
    const nextIdentity = `${query.data.relayPubkey}:${query.data.projectionGeneration}`;
    const previousIdentity = previousIdentityRef.current;
    previousIdentityRef.current = nextIdentity;
    if (previousIdentity === null || previousIdentity === nextIdentity) return;
    queryClient.removeQueries({
      predicate: (candidate) =>
        isDocumentBusinessKeyForCommunity(candidate.queryKey, communityKey),
    });
  }, [communityKey, query.data, queryClient]);

  return query;
}

export function useProjectDocuments(meta?: ProjectDocumentMeta) {
  return useQuery({
    queryKey: meta
      ? projectDocumentsKey(meta)
      : ["project-documents", "pending"],
    queryFn: () => listProjectDocuments(meta as ProjectDocumentMeta),
    enabled: Boolean(meta),
    placeholderData: (previous) =>
      meta &&
      previous?.communityKey === meta.communityKey &&
      previous.projectId === meta.projectId &&
      previous.relayPubkey === meta.relayPubkey &&
      previous.projectionGeneration === meta.projectionGeneration
        ? previous
        : undefined,
    staleTime: 15_000,
  });
}

export function useProjectDocument(input: {
  identity?: ProjectDocumentIdentity;
  documentId?: string;
  revision?: number;
  enabled?: boolean;
}) {
  const revision = input.revision ?? "current";
  return useQuery({
    queryKey:
      input.identity && input.documentId
        ? projectDocumentKey(input.identity, input.documentId, revision)
        : ["project-document", "pending"],
    queryFn: () =>
      getProjectDocument({
        identity: input.identity as ProjectDocumentIdentity,
        documentId: input.documentId as string,
        revision: input.revision,
      }),
    enabled: Boolean(
      input.identity && input.documentId && (input.enabled ?? true),
    ),
    staleTime: revision === "current" ? 15_000 : Number.POSITIVE_INFINITY,
  });
}

export function useProjectDocumentHistory(input: {
  identity?: ProjectDocumentIdentity;
  documentId?: string;
  maxDocumentRevision?: number;
  enabled?: boolean;
}) {
  return useQuery({
    queryKey:
      input.identity && input.documentId && input.maxDocumentRevision
        ? projectDocumentHistoryKey(
            input.identity,
            input.documentId,
            input.maxDocumentRevision,
          )
        : ["project-document-history", "pending"],
    queryFn: () =>
      getProjectDocumentHistory({
        identity: input.identity as ProjectDocumentIdentity,
        documentId: input.documentId as string,
        maxDocumentRevision: input.maxDocumentRevision as number,
      }),
    enabled: Boolean(
      input.identity &&
        input.documentId &&
        input.maxDocumentRevision &&
        (input.enabled ?? true),
    ),
    staleTime: 15_000,
  });
}

export function useProjectDocumentMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (input: {
      identity: ProjectDocumentIdentity;
      mutation: ProjectDocumentMutation;
    }) => mutateProjectDocument(input),
    onSuccess: async (result, input) => {
      const communityKey = input.identity.communityKey;
      const invalidation = Promise.all([
        queryClient.invalidateQueries({
          predicate: (candidate) =>
            candidate.queryKey[0] === "project-document-meta" &&
            candidate.queryKey[1] === communityKey,
        }),
        queryClient.invalidateQueries({
          predicate: (candidate) =>
            candidate.queryKey[0] === "project-documents" &&
            candidate.queryKey[1] === communityKey,
        }),
        queryClient.invalidateQueries({
          predicate: (candidate) =>
            candidate.queryKey[0] === "project-document" &&
            candidate.queryKey[1] === communityKey &&
            candidate.queryKey[5] === result.documentId &&
            candidate.queryKey[6] === "current",
        }),
        queryClient.invalidateQueries({
          predicate: (candidate) =>
            candidate.queryKey[0] === "project-document-history" &&
            candidate.queryKey[1] === communityKey &&
            candidate.queryKey[5] === result.documentId,
        }),
      ]);
      if (result.status === "conflict") {
        void invalidation;
        return;
      }
      await invalidation;
    },
  });
}

export type ProjectDocumentLiveStatus =
  | "idle"
  | "connecting"
  | "live"
  | "retrying";

export function useProjectDocumentLiveSync(
  meta?: ProjectDocumentMeta,
): ProjectDocumentLiveStatus {
  const queryClient = useQueryClient();
  const [status, setStatus] = React.useState<ProjectDocumentLiveStatus>("idle");
  const invalidate = React.useEffectEvent(async (communityKey: string) => {
    await Promise.all([
      queryClient.invalidateQueries({
        predicate: (candidate) =>
          candidate.queryKey[0] === "project-document-meta" &&
          candidate.queryKey[1] === communityKey,
      }),
      queryClient.invalidateQueries({
        predicate: (candidate) =>
          candidate.queryKey[0] === "project-documents" &&
          candidate.queryKey[1] === communityKey,
      }),
      queryClient.invalidateQueries({
        predicate: (candidate) =>
          candidate.queryKey[0] === "project-document" &&
          candidate.queryKey[1] === communityKey &&
          candidate.queryKey[6] === "current",
      }),
      queryClient.invalidateQueries({
        predicate: (candidate) =>
          candidate.queryKey[0] === "project-document-history" &&
          candidate.queryKey[1] === communityKey,
      }),
    ]);
  });
  const liveCommunityKey = meta?.communityKey;
  const liveRelayPubkey = meta?.relayPubkey;
  const liveSnapshotUpdatedAt = meta?.updatedAt;

  React.useEffect(() => {
    if (!liveCommunityKey || !liveRelayPubkey || !liveSnapshotUpdatedAt) {
      setStatus("idle");
      return;
    }
    let cancelled = false;
    let retryAttempt = 0;
    let retryTimer: number | null = null;
    let disposeSubscription: (() => Promise<void>) | undefined;
    const scheduler = new ProjectDocumentInvalidationScheduler(
      () => (cancelled ? undefined : invalidate(liveCommunityKey)),
      undefined,
      window.setTimeout.bind(window),
      window.clearTimeout.bind(window),
    );

    const subscribe = async () => {
      if (cancelled) return;
      setStatus(retryAttempt === 0 ? "connecting" : "retrying");
      try {
        const dispose = await relayClient.subscribeLive(
          projectDocumentLiveFilter({
            relayPubkey: liveRelayPubkey,
            snapshotUpdatedAt: liveSnapshotUpdatedAt,
          }),
          () => scheduler.signal(),
        );
        if (cancelled) {
          void dispose().catch(() => {});
          return;
        }
        disposeSubscription = dispose;
        retryAttempt = 0;
        setStatus("live");
        scheduler.signal();
      } catch {
        if (cancelled) return;
        setStatus("retrying");
        const delay = Math.min(30_000, 1_000 * 2 ** Math.min(retryAttempt, 5));
        retryAttempt += 1;
        retryTimer = window.setTimeout(() => void subscribe(), delay);
      }
    };
    void subscribe();
    return () => {
      cancelled = true;
      scheduler.dispose();
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      if (disposeSubscription) void disposeSubscription().catch(() => {});
    };
  }, [liveCommunityKey, liveRelayPubkey, liveSnapshotUpdatedAt]);

  return status;
}

export function identityFromMeta(
  meta: ProjectDocumentMeta,
): ProjectDocumentIdentity {
  return documentIdentity(meta);
}
