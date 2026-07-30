import * as React from "react";

import {
  useManagedAgentsQuery,
  useRelayAgentsQuery,
} from "@/features/agents/hooks";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import { indexProjectViewObjects } from "@/features/project-view/model";
import type {
  ProjectView,
  ProjectViewRoleContinuity,
} from "@/shared/api/tauriProjectView";
import { useIdentityQuery } from "@/shared/api/hooks";

/**
 * Resolves the people and agents referenced by one verified Project View
 * snapshot. Both the Community overview and full View use this hook so actor
 * labels cannot drift between the two presentation densities.
 */
export function useProjectViewActors(
  view: ProjectView,
  roleContinuity?: ProjectViewRoleContinuity,
) {
  const objectsById = React.useMemo(
    () => indexProjectViewObjects(view),
    [view],
  );
  const actorPubkeys = React.useMemo(() => {
    const pubkeys = new Set<string>();
    for (const object of objectsById.values()) {
      pubkeys.add(object.createdBy);
      pubkeys.add(object.updatedBy);
    }
    if (roleContinuity) {
      for (const member of roleContinuity.members) pubkeys.add(member.pubkey);
      for (const assignment of roleContinuity.assignments) {
        pubkeys.add(assignment.memberPubkey);
        pubkeys.add(assignment.startedBy);
        if (assignment.endedBy) pubkeys.add(assignment.endedBy);
      }
      for (const proposal of roleContinuity.proposals) {
        pubkeys.add(proposal.candidatePubkey);
        pubkeys.add(proposal.createdBy);
        if (proposal.authorizedBy) pubkeys.add(proposal.authorizedBy);
      }
    }
    return [...pubkeys];
  }, [objectsById, roleContinuity]);
  const actorProfilesQuery = useUsersBatchQuery(actorPubkeys);
  const managedAgentsQuery = useManagedAgentsQuery();
  const relayAgentsQuery = useRelayAgentsQuery();
  const identityQuery = useIdentityQuery();
  const actorProfiles = React.useMemo<UserProfileLookup>(() => {
    const profiles = { ...(actorProfilesQuery.data?.profiles ?? {}) };
    const actorSet = new Set(
      actorPubkeys.map((pubkey) => pubkey.toLowerCase()),
    );
    const knownAgents = [
      ...(relayAgentsQuery.data ?? []),
      ...(managedAgentsQuery.data ?? []),
    ];
    for (const agent of knownAgents) {
      const pubkey = agent.pubkey.toLowerCase();
      if (!actorSet.has(pubkey)) continue;
      const existing = profiles[pubkey];
      profiles[pubkey] = {
        avatarUrl:
          existing?.avatarUrl ??
          ("avatarUrl" in agent ? agent.avatarUrl : null),
        displayName: existing?.displayName ?? agent.name,
        isAgent: true,
        name: existing?.name ?? agent.name,
        nip05Handle: existing?.nip05Handle ?? null,
        ownerPubkey: existing?.ownerPubkey ?? null,
      };
    }
    return profiles;
  }, [
    actorProfilesQuery.data?.profiles,
    actorPubkeys,
    managedAgentsQuery.data,
    relayAgentsQuery.data,
  ]);

  return {
    actorProfiles,
    currentPubkey: identityQuery.data?.pubkey,
    objectsById,
  };
}
