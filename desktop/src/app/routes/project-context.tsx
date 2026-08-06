import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import {
  projectContextRouteSearchForState,
  projectContextRouteStateFromSearch,
  validateProjectContextRouteSearch,
  type ProjectContextRouteSearch,
} from "@/features/project-context/routeState";
import { usePreviewFeatureWarning } from "@/shared/features";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

const ProjectContextScreen = React.lazy(async () => {
  const module = await import(
    "@/features/project-context/ui/ProjectContextScreen"
  );
  return { default: module.ProjectContextScreen };
});

export type { ProjectContextRouteSearch } from "@/features/project-context/routeState";

/** Reject malformed stable query or selection tokens before native is called. */
export function validateProjectContextSearch(
  search: Record<string, unknown>,
): ProjectContextRouteSearch {
  return validateProjectContextRouteSearch(search);
}

export const Route = createFileRoute("/project-context")({
  validateSearch: validateProjectContextSearch,
  component: ProjectContextRouteComponent,
});

function ProjectContextRouteComponent() {
  usePreviewFeatureWarning("projectView");
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  if (search.invalid) {
    return (
      <React.Suspense fallback={<ViewLoadingFallback kind="view" />}>
        <ProjectContextScreen
          onResetRoute={() => void navigate({ search: {}, replace: true })}
          routeError={search.invalid}
        />
      </React.Suspense>
    );
  }
  const state = projectContextRouteStateFromSearch(search);
  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="view" />}>
      <ProjectContextScreen
        appliedQuery={state.query}
        onApplyQuery={(query) =>
          void navigate({
            search: projectContextRouteSearchForState(query),
          })
        }
        onSelectionChange={(selection, options) =>
          void navigate({
            search: projectContextRouteSearchForState(state.query, selection),
            replace: options?.replace,
          })
        }
        selection={state.selection}
      />
    </React.Suspense>
  );
}
