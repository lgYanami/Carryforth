import { AlertTriangle } from "lucide-react";

import { ProjectContextHeader } from "@/features/project-context/ui/ProjectContextHeader";
import { Button } from "@/shared/ui/button";

/** Rejected Project Context deep-link surface with no trusted query side effect. */
export function ProjectContextInvalidRoute({
  onResetRoute,
  routeError,
}: {
  onResetRoute: () => void;
  routeError: string;
}) {
  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="project-context-screen"
    >
      <ProjectContextHeader />
      <main
        className="flex min-h-0 flex-1 items-center justify-center p-6"
        data-testid="project-context-invalid-route"
      >
        <div className="max-w-lg text-center">
          <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-2xl border border-destructive/30 bg-destructive/10 text-destructive">
            <AlertTriangle className="h-5 w-5" />
          </div>
          <h1 className="mt-4 text-lg font-semibold">
            Project Context link is invalid
          </h1>
          <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
            The query or selection in this link was rejected before Desktop
            contacted the trusted Project Context boundary.
          </p>
          <code className="mt-4 block rounded-lg border border-border/70 bg-muted/20 px-3 py-2 text-left text-xs text-muted-foreground">
            {routeError}
          </code>
          <Button
            className="mt-4"
            data-testid="project-context-reset-invalid-route"
            onClick={onResetRoute}
            size="sm"
            type="button"
            variant="outline"
          >
            Open All Context
          </Button>
        </div>
      </main>
    </div>
  );
}
