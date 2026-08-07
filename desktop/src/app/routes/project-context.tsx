import * as React from "react";
import {
  createFileRoute,
  type ErrorComponentProps,
} from "@tanstack/react-router";
import { AlertTriangle } from "lucide-react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  projectContextRouteSearchForState,
  projectContextRouteStateFromSearch,
  validateProjectContextRouteSearch,
  type ProjectContextRouteSearch,
} from "@/features/project-context/routeState";
import { usePreviewFeatureWarning } from "@/shared/features";
import { Button } from "@/shared/ui/button";
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
  errorComponent: ProjectContextRouteError,
});

function ProjectContextRouteError({ error, reset }: ErrorComponentProps) {
  const navigate = Route.useNavigate();
  const resetQuery = React.useCallback(() => {
    void navigate({ search: {}, replace: true }).finally(reset);
  }, [navigate, reset]);

  return (
    <main
      className="flex min-h-0 min-w-0 flex-1 items-center justify-center overflow-auto p-6"
      data-testid="project-context-route-error"
    >
      <section className="w-full max-w-lg rounded-xl border border-destructive/30 bg-card p-5 shadow-sm">
        <div className="flex items-start gap-3">
          <AlertTriangle className="mt-0.5 h-5 w-5 shrink-0 text-destructive" />
          <div className="min-w-0 flex-1">
            <h1 className="text-base font-semibold">
              Project Context needs to recover
            </h1>
            <p className="mt-1 text-sm text-muted-foreground">
              This view encountered a local error. Your project data was not
              changed.
            </p>
            <p className="mt-3 break-words rounded-lg bg-muted/50 p-3 font-mono text-xs text-muted-foreground">
              {error.message || "Unknown Project Context error"}
            </p>
            <div className="mt-4 flex flex-wrap gap-2">
              <Button onClick={reset} type="button" variant="outline">
                Retry Project Context
              </Button>
              <Button
                data-testid="project-context-reset-route-error"
                onClick={resetQuery}
                type="button"
              >
                Reset query
              </Button>
            </div>
          </div>
        </div>
      </section>
    </main>
  );
}

function ProjectContextRouteComponent() {
  usePreviewFeatureWarning("projectView");
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const { goChannel, goDocuments, goView } = useAppNavigation();
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
        onOpenDocument={(documentId) => void goDocuments({ documentId })}
        onOpenMeeting={(meetingId) => void goChannel(meetingId)}
        onOpenProjectView={(objectId) => void goView({ objectId })}
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
