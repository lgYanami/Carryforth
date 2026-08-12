import * as React from "react";

import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import type { ProjectContextSemanticOverlay } from "@/features/project-context/semanticOverlay";
import { ProjectContextGraph } from "@/features/project-context/ui/ProjectContextGraph";
import type { ProjectContextWorkspaceCanvasRenderContext } from "@/features/project-context/ui/ProjectContextWorkspace";
import { projectContextWorkspaceSelectionKey } from "@/features/project-context/workspacePanelModel";
import type { ProjectContextQueryResult } from "@/shared/api/tauriProjectContext";

/** Graph adapter that records presentation-only click origins before route updates. */
export function ProjectContextWorkspaceGraph({
  canvas,
  fitSemanticPathsRequest,
  focusSelectionRequest,
  onClearSemanticResult,
  onSelectionChange,
  result,
  selection,
  semanticFreshness,
  semanticOverlay,
  semanticSessionOverlay,
}: {
  canvas: ProjectContextWorkspaceCanvasRenderContext;
  fitSemanticPathsRequest: number;
  focusSelectionRequest: number;
  onClearSemanticResult: () => void;
  onSelectionChange: (
    selection: ProjectContextRouteSelection | null,
    options?: { replace?: boolean },
  ) => void;
  result: ProjectContextQueryResult;
  selection: ProjectContextRouteSelection | null;
  semanticFreshness: "snapshot" | "stale";
  semanticOverlay: ProjectContextSemanticOverlay | null;
  semanticSessionOverlay: ProjectContextSemanticOverlay | null;
}) {
  const {
    externalCanvasInsets,
    fitSuspended,
    registerSelectionOpenIntent,
    rejectSelectionOpenIntent,
  } = canvas;
  const handleSelectionChange = React.useCallback(
    (
      next: ProjectContextRouteSelection | null,
      options?: { replace?: boolean },
    ) => {
      if (next) {
        const selectionKey = projectContextWorkspaceSelectionKey(next);
        registerSelectionOpenIntent(selectionKey, next);
      } else {
        rejectSelectionOpenIntent();
      }
      onSelectionChange(next, options);
      window.requestAnimationFrame(rejectSelectionOpenIntent);
    },
    [onSelectionChange, registerSelectionOpenIntent, rejectSelectionOpenIntent],
  );

  return (
    <main
      className="flex h-full min-h-0 min-w-0 flex-1 overflow-hidden"
      data-testid="project-context-graph-slot"
      tabIndex={-1}
    >
      <ProjectContextGraph
        externalCanvasInsets={externalCanvasInsets}
        fitSemanticPathsRequest={fitSemanticPathsRequest}
        fitSuspended={fitSuspended}
        focusSelectionRequest={focusSelectionRequest}
        onClearSemanticResult={onClearSemanticResult}
        onSelectionChange={handleSelectionChange}
        result={result}
        selection={selection}
        semanticFreshness={semanticFreshness}
        semanticOverlay={semanticOverlay}
        semanticSessionOverlay={semanticSessionOverlay}
      />
    </main>
  );
}
