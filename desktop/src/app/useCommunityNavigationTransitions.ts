import { useRouter } from "@tanstack/react-router";
import * as React from "react";

import type { deriveShellRoute } from "@/app/AppShell.helpers";
import type { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  replaceCommunityOverviewRoute,
  runCommunityViewTransition,
} from "@/app/communityViewTransition";
import {
  communityDestinationFromRoute,
  saveCommunityDestination,
} from "@/features/communities/communityNavigationStorage";
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
    const destination = communityDestinationFromRoute(
      selectedView,
      selectedChannelId,
    );
    if (destination) {
      saveCommunityDestination(activeCommunityId, destination);
    }
  }, [communities.activeCommunity?.id, selectedChannelId, selectedView]);

  const openCommunityOverview = React.useCallback(async () => {
    saveActiveDestination();
    await goCommunity();
  }, [goCommunity, saveActiveDestination]);

  // Home is a teardown barrier: the outgoing channel must unmount before the
  // relay changes, or its read effect can advance markers on the wrong relay.
  const switchCommunity = React.useCallback(
    async (id: string) => {
      const activeCommunityId = communities.activeCommunity?.id;
      if (id === activeCommunityId) {
        await openCommunityOverview();
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
    [
      communities,
      goHome,
      openCommunityOverview,
      router.history,
      saveActiveDestination,
    ],
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

  return { openCommunityOverview, removeCommunity, switchCommunity };
}
