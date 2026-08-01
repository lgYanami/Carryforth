import * as React from "react";

import { useChannelsQuery } from "@/features/channels/hooks";
import {
  loadCommunityDestination,
  saveCommunityDestination,
} from "@/features/communities/communityNavigationStorage";
import { resolveCommunityContinueTarget } from "@/features/communities/communityContinueTarget";
import { useCommunities } from "@/features/communities/useCommunities";

/**
 * Resolves the active Community's remembered work position for the Overview.
 *
 * A cached channel snapshot is intentionally not authoritative: a remembered
 * channel is offered only after the active relay has validated current
 * membership and archive state.
 */
export function useCommunityContinueTarget() {
  const { activeCommunity } = useCommunities();
  const channelsQuery = useChannelsQuery();
  const communityId = activeCommunity?.id;
  const destination = communityId
    ? loadCommunityDestination(communityId)
    : null;
  const resolution = resolveCommunityContinueTarget(
    destination,
    channelsQuery.data ?? [],
    channelsQuery.isSuccess && channelsQuery.dataUpdatedAt > 0,
  );

  React.useEffect(() => {
    if (!communityId || resolution.status !== "invalid") {
      return;
    }
    saveCommunityDestination(communityId, { kind: "home" });
  }, [communityId, resolution.status]);

  return resolution;
}
