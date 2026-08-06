import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";

import { usePreviewFeatureWarning } from "@/shared/features";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";

const ProjectContextScreen = React.lazy(async () => {
  const module = await import(
    "@/features/project-context/ui/ProjectContextScreen"
  );
  return { default: module.ProjectContextScreen };
});

export type ProjectContextRouteSearch = Record<string, never>;

/** Stage-two route accepts only the canonical omitted search for All Context. */
export function validateProjectContextSearch(
  _search: Record<string, unknown>,
): ProjectContextRouteSearch {
  return {};
}

export const Route = createFileRoute("/project-context")({
  validateSearch: validateProjectContextSearch,
  component: ProjectContextRouteComponent,
});

function ProjectContextRouteComponent() {
  usePreviewFeatureWarning("projectView");
  return (
    <React.Suspense fallback={<ViewLoadingFallback kind="view" />}>
      <ProjectContextScreen />
    </React.Suspense>
  );
}
