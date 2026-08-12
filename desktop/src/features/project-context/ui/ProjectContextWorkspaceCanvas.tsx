import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import type { ProjectContextSemanticOverlay } from "@/features/project-context/semanticOverlay";
import {
  projectContextErrorMessage,
  projectContextFailureKind,
  type ProjectContextFailureKind,
} from "@/features/project-context/state";
import {
  ProjectContextEmptyState,
  ProjectContextFailureState,
  ProjectContextLoadingState,
} from "@/features/project-context/ui/ProjectContextStates";
import type { ProjectContextWorkspaceCanvasRenderContext } from "@/features/project-context/ui/ProjectContextWorkspace";
import { ProjectContextWorkspaceGraph } from "@/features/project-context/ui/ProjectContextWorkspaceGraph";
import type { ProjectContextQueryResult } from "@/shared/api/tauriProjectContext";

/** Full-canvas loading, failure, empty, or verified graph substrate. */
export function ProjectContextWorkspaceCanvas({
  canvas,
  displayedAllContext,
  failure,
  failureKind,
  fitSemanticPathsRequest,
  focusSelectionRequest,
  onClearSemanticResult,
  onRetry,
  onSelectionChange,
  pending,
  result,
  retrying,
  selection,
  semanticFreshness,
  semanticOverlay,
  semanticSessionOverlay,
}: {
  canvas: ProjectContextWorkspaceCanvasRenderContext;
  displayedAllContext: boolean;
  failure?: unknown;
  failureKind?: ProjectContextFailureKind;
  fitSemanticPathsRequest: number;
  focusSelectionRequest: number;
  onClearSemanticResult: () => void;
  onRetry: () => void;
  onSelectionChange: (
    selection: ProjectContextRouteSelection | null,
    options?: { replace?: boolean },
  ) => void;
  pending: boolean;
  result?: ProjectContextQueryResult;
  retrying: boolean;
  selection: ProjectContextRouteSelection | null;
  semanticFreshness: "snapshot" | "stale";
  semanticOverlay: ProjectContextSemanticOverlay | null;
  semanticSessionOverlay: ProjectContextSemanticOverlay | null;
}) {
  if (failure) {
    return (
      <ProjectContextFailureState
        diagnostic={projectContextErrorMessage(failure)}
        kind={failureKind ?? projectContextFailureKind(failure)}
        onRetry={onRetry}
        retrying={retrying}
      />
    );
  }

  if (pending && !result) return <ProjectContextLoadingState />;

  if (result && displayedAllContext && result.context.activeEdgeCount === 0) {
    return <ProjectContextEmptyState />;
  }

  if (!result) return <ProjectContextLoadingState />;

  return (
    <ProjectContextWorkspaceGraph
      canvas={canvas}
      fitSemanticPathsRequest={fitSemanticPathsRequest}
      focusSelectionRequest={focusSelectionRequest}
      onClearSemanticResult={onClearSemanticResult}
      onSelectionChange={onSelectionChange}
      result={result}
      selection={selection}
      semanticFreshness={semanticFreshness}
      semanticOverlay={semanticOverlay}
      semanticSessionOverlay={semanticSessionOverlay}
    />
  );
}
