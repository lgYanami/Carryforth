import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

const CommunityProjectOverviewScreen = React.lazy(async () => {
  const module = await import(
    "@/features/project-view/ui/CommunityProjectOverviewScreen"
  );
  return { default: module.CommunityProjectOverviewScreen };
});

export const Route = createFileRoute("/community")({
  component: CommunityRouteComponent,
});

function CommunityRouteComponent() {
  const { goChannel, goHome, goSettings, goView } = useAppNavigation();

  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="view" />}>
      <CommunityProjectOverviewScreen
        onOpenChannel={(channelId) => void goChannel(channelId)}
        onOpenExperiments={() => void goSettings("experimental")}
        onOpenFullView={() => void goView()}
        onOpenInbox={() => void goHome()}
        onOpenObject={(objectId) => void goView({ objectId })}
      />
    </React.Suspense>
  );
}
