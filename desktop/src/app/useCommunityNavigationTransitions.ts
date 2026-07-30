import { useRouter } from "@tanstack/react-router";
import * as React from "react";

import type { deriveShellRoute } from "@/app/AppShell.helpers";
import type { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  replaceCommunityOverviewRoute,
  runCommunityViewTransition,
} from "@/app/communityViewTransition";
import { saveCommunityDestination } from "@/features/communities/communityNavigationStorage";
import type { useCommunities } from "@/features/communities/useCommunities";

type Communities = ReturnType<typeof useCommunities>;
type ShellRoute = ReturnType<typeof deriveShellRoute>;
type GoHome = ReturnType<typeof useAppNavigation>["goHome"];
type GoCommunity = ReturnType<typeof useAppNavigation>["goCommunity"];

export function useCommunityNavigationTransitions({
  communities,
  goCommunity,
  goHome,
  selectedChannelId,
  selectedView,
}: {
  communities: Communities;
  goCommunity: GoCommunity;
  goHome: GoHome;
  selectedChannelId: ShellRoute["selectedChannelId"];
  selectedView: ShellRoute["selectedView"];
}) {
  const router = useRouter();
  const saveActiveDestination = React.useCallback(() => {
    const activeCommunityId = communities.activeCommunity?.id;
    if (!activeCommunityId) return;
    saveCommunityDestination(
      activeCommunityId,
      selectedView === "channel" && selectedChannelId
        ? { kind: "channel", channelId: selectedChannelId }
        : { kind: "home" },
    );
  }, [communities.activeCommunity?.id, selectedChannelId, selectedView]);

  // Home is a teardown barrier: the outgoing channel must unmount before the
  // relay changes, or its read effect can advance markers on the wrong relay.
  const switchCommunity = React.useCallback(
    async (id: string) => {
      const activeCommunityId = communities.activeCommunity?.id;
      if (id === activeCommunityId) {
        await goCommunity();
        return;
      }
      if (!activeCommunityId) {
        replaceCommunityOverviewRoute(router.history);
        communities.switchCommunity(id);
        return;
      }

      await runCommunityViewTransition(async () => {
        saveActiveDestination();
        await goHome({ replace: true });
        replaceCommunityOverviewRoute(router.history);
        communities.switchCommunity(id);
      });
    },
    [communities, goCommunity, goHome, router.history, saveActiveDestination],
  );

  const removeCommunity = React.useCallback(
    async (id: string) => {
      if (id !== communities.activeCommunity?.id) {
        communities.removeCommunity(id);
        return;
      }
      const fallback = communities.communities.find(
        (community) => community.id !== id,
      );
      if (!fallback) return;

      await runCommunityViewTransition(async () => {
        saveActiveDestination();
        await goHome({ replace: true });
        replaceCommunityOverviewRoute(router.history);
        communities.removeCommunity(id);
      });
    },
    [communities, goHome, router.history, saveActiveDestination],
  );

  return { removeCommunity, switchCommunity };
}
