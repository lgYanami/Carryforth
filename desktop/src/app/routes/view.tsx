import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import type { ProjectViewExplorerSelection } from "@/features/project-view/explorerModel";
import {
  projectViewRouteForSelection,
  projectViewSelectionFromRoute,
  validateProjectViewRouteSearch,
} from "@/features/project-view/routeSelection";
import { usePreviewFeatureWarning } from "@/shared/features";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

const ProjectViewScreen = React.lazy(async () => {
  const module = await import("@/features/project-view/ui/ProjectViewScreen");
  return { default: module.ProjectViewScreen };
});

export const Route = createFileRoute("/view")({
  validateSearch: validateProjectViewRouteSearch,
  component: ViewRouteComponent,
});

function ViewRouteComponent() {
  usePreviewFeatureWarning("projectView");
  const { goCommunity, goProjectContext } = useAppNavigation();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();

  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="view" />}>
      <ProjectViewScreen
        onOpenOverview={() => void goCommunity()}
        onOpenDocument={(documentSearch) =>
          void navigate({
            to: "/documents",
            search: documentSearch,
          })
        }
        onSelectItem={(
          selection: ProjectViewExplorerSelection | undefined,
          options?: { replace?: boolean },
        ) =>
          void navigate({
            replace: options?.replace,
            search: projectViewRouteForSelection(selection),
          })
        }
        onShowInProjectContext={(object) =>
          void goProjectContext({
            query: {
              type: "incident",
              coordinate: {
                type: "project_view_object",
                objectType: object.objectType,
                objectId: object.id,
              },
            },
          })
        }
        selection={projectViewSelectionFromRoute(search)}
      />
    </React.Suspense>
  );
}
