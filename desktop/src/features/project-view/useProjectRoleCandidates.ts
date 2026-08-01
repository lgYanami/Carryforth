import * as React from "react";

import {
  useManagedAgentsQuery,
  useRelayAgentsQuery,
} from "@/features/agents/hooks";
import { useManagedAgentRuntimesQuery } from "@/features/agents/managedAgentRuntimeHooks";
import { useCommunities } from "@/features/communities/useCommunities";
import { useArchivedIdentitiesQuery } from "@/features/identity-archive/hooks";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import { buildProjectRoleCandidates } from "@/features/project-view/projectRoleCandidates";
import type { ProjectViewRoleContinuity } from "@/shared/api/tauriProjectView";
import { useIdentityQuery } from "@/shared/api/hooks";
import { normalizePubkey } from "@/shared/lib/pubkey";

type UseProjectRoleCandidatesInput = {
  actorProfiles?: UserProfileLookup;
  continuity: ProjectViewRoleContinuity;
  currentPubkey?: string;
  enabled: boolean;
  targetRoleId: string;
};

/**
 * Composes the current Community's member, Relay Agent, and local managed
 * Agent sources into the Human-facing Role candidate directory.
 */
export function useProjectRoleCandidates({
  actorProfiles,
  continuity,
  currentPubkey,
  enabled,
  targetRoleId,
}: UseProjectRoleCandidatesInput) {
  const { activeCommunity } = useCommunities();
  const identityQuery = useIdentityQuery();
  const managedAgentsQuery = useManagedAgentsQuery({ enabled });
  const managedAgentRuntimesQuery = useManagedAgentRuntimesQuery({ enabled });
  const relayAgentsQuery = useRelayAgentsQuery({ enabled });
  const archivedIdentitiesQuery = useArchivedIdentitiesQuery(enabled);
  const resolvedCurrentPubkey = currentPubkey ?? identityQuery.data?.pubkey;

  const profilePubkeys = React.useMemo(() => {
    const pubkeys = new Set<string>();
    for (const member of continuity.members) {
      pubkeys.add(normalizePubkey(member.pubkey));
    }
    for (const agent of managedAgentsQuery.data ?? []) {
      pubkeys.add(normalizePubkey(agent.pubkey));
    }
    for (const agent of relayAgentsQuery.data ?? []) {
      pubkeys.add(normalizePubkey(agent.pubkey));
    }
    if (resolvedCurrentPubkey) {
      pubkeys.add(normalizePubkey(resolvedCurrentPubkey));
    }
    return [...pubkeys];
  }, [
    continuity.members,
    managedAgentsQuery.data,
    relayAgentsQuery.data,
    resolvedCurrentPubkey,
  ]);
  const profilesQuery = useUsersBatchQuery(profilePubkeys, { enabled });

  const profiles = React.useMemo<UserProfileLookup>(
    () => ({
      ...(actorProfiles ?? {}),
      ...(profilesQuery.data?.profiles ?? {}),
    }),
    [actorProfiles, profilesQuery.data?.profiles],
  );
  const archivedPubkeys = React.useMemo(
    () =>
      new Set(
        (archivedIdentitiesQuery.data?.archived ?? []).map((pubkey) =>
          normalizePubkey(pubkey),
        ),
      ),
    [archivedIdentitiesQuery.data?.archived],
  );
  const candidates = React.useMemo(
    () =>
      buildProjectRoleCandidates({
        activeRelayUrl: activeCommunity?.relayUrl,
        archivedPubkeys,
        assignments: continuity.assignments,
        currentPubkey: resolvedCurrentPubkey,
        managedAgents: managedAgentsQuery.data ?? [],
        managedAgentRuntimes: managedAgentRuntimesQuery.data ?? [],
        members: continuity.members,
        profiles,
        proposals: continuity.proposals,
        relayAgents: relayAgentsQuery.data ?? [],
        roles: continuity.roles,
        targetRoleId,
      }),
    [
      activeCommunity?.relayUrl,
      archivedPubkeys,
      continuity.assignments,
      continuity.members,
      continuity.proposals,
      continuity.roles,
      managedAgentsQuery.data,
      managedAgentRuntimesQuery.data,
      profiles,
      relayAgentsQuery.data,
      resolvedCurrentPubkey,
      targetRoleId,
    ],
  );

  const sourceQueries = [
    managedAgentsQuery,
    managedAgentRuntimesQuery,
    relayAgentsQuery,
    profilesQuery,
    archivedIdentitiesQuery,
  ];
  const firstError = sourceQueries.find((query) => query.error)?.error;

  return {
    candidates,
    error: firstError instanceof Error ? firstError : null,
    isLoading:
      enabled &&
      candidates.length === 0 &&
      sourceQueries.some((query) => query.isLoading),
    isPartial: sourceQueries.some((query) => query.isError),
  };
}
