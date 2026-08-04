import * as React from "react";

import {
  consumePendingCommunityRestore,
  loadCommunityDestination,
  saveCommunityDestination,
} from "@/features/communities/communityNavigationStorage";
import type { Channel } from "@/shared/api/types";

type RestoreNavigation = {
  goChannel: (channelId: string, options?: { replace?: boolean }) => unknown;
  goHome: (options?: { replace?: boolean }) => unknown;
};

/** Restore a remembered ordinary Channel or Meeting after Community switch. */
export function useCommunityDestinationRestore(input: {
  activeCommunityId?: string;
  channelsDataUpdatedAt: number;
  channelsReady: boolean;
  meetingRooms: readonly Channel[];
  selectedView: string;
  sidebarChannels: readonly Channel[];
  navigation: RestoreNavigation;
}): void {
  const hasRestoredRef = React.useRef(false);
  const {
    activeCommunityId,
    channelsDataUpdatedAt,
    channelsReady,
    meetingRooms,
    selectedView,
    sidebarChannels,
  } = input;
  const { goChannel, goHome } = input.navigation;

  React.useEffect(() => {
    if (
      hasRestoredRef.current ||
      !channelsReady ||
      channelsDataUpdatedAt === 0 ||
      !activeCommunityId
    ) {
      return;
    }
    hasRestoredRef.current = true;

    // Restoration belongs to an explicit community transition. Cold boot and
    // reconnect remounts preserve the route the user explicitly opened.
    if (!consumePendingCommunityRestore(activeCommunityId)) return;

    const destination = loadCommunityDestination(activeCommunityId);
    if (!destination || destination.kind === "home") return;

    const channelIsAvailable = [...sidebarChannels, ...meetingRooms].some(
      (channel) => channel.id === destination.channelId,
    );
    if (!channelIsAvailable) {
      saveCommunityDestination(activeCommunityId, { kind: "home" });
      void goHome({ replace: true });
      return;
    }

    // Onboarding and deep-link transitions may request restoration after the
    // target Community mounts. Normal rail navigation exposes Continue.
    if (selectedView === "home") {
      void goChannel(destination.channelId, { replace: true });
    }
  }, [
    activeCommunityId,
    channelsDataUpdatedAt,
    channelsReady,
    goChannel,
    goHome,
    meetingRooms,
    selectedView,
    sidebarChannels,
  ]);
}
