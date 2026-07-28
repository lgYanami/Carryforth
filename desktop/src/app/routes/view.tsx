import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import { usePreviewFeatureWarning } from "@/shared/features";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

const ProjectViewScreen = React.lazy(async () => {
  const module = await import("@/features/project-view/ui/ProjectViewScreen");
  return { default: module.ProjectViewScreen };
});

type ViewRouteSearch = {
  object?: string;
};

function validateViewSearch(search: Record<string, unknown>): ViewRouteSearch {
  return {
    object:
      typeof search.object === "string" && search.object.length > 0
        ? search.object
        : undefined,
  };
}

export const Route = createFileRoute("/view")({
  validateSearch: validateViewSearch,
  component: ViewRouteComponent,
});

function ViewRouteComponent() {
  usePreviewFeatureWarning("projectView");
  const search = Route.useSearch();
  const navigate = Route.useNavigate();

  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="view" />}>
      <ProjectViewScreen
        onSelectObject={(object) =>
          void navigate({
            search: (previous) => ({
              ...previous,
              object,
            }),
          })
        }
        selectedObjectId={search.object}
      />
    </React.Suspense>
  );
}
