import { LocateFixed } from "lucide-react";
import * as React from "react";

import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import type { ProjectContextWorkspaceAnnouncementEvent } from "@/features/project-context/workspacePanelModel";
import { ProjectContextInspectorContent } from "@/features/project-context/ui/ProjectContextInspectorContent";
import type {
  ProjectContextCoordinate,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import type { ProjectViewLoadResult } from "@/shared/api/tauriProjectView";
import { useEscapeKey } from "@/shared/hooks/useEscapeKey";
import {
  AuxiliaryPanel,
  AuxiliaryPanelBody,
  AuxiliaryPanelHeader,
  AuxiliaryPanelHeaderActions,
  AuxiliaryPanelHeaderGroup,
  AuxiliaryPanelHeaderTitleBlock,
  clampAuxiliaryPanelWidth,
} from "@/shared/layout/AuxiliaryPanel";
import { Button } from "@/shared/ui/button";

const PROJECT_CONTEXT_INSPECTOR_WIDTH_PX = 440;

function useProjectContextInspectorWidth() {
  const [widthPx, setWidthPx] = React.useState(
    PROJECT_CONTEXT_INSPECTOR_WIDTH_PX,
  );
  const cleanupRef = React.useRef<(() => void) | null>(null);

  React.useEffect(() => () => cleanupRef.current?.(), []);

  const onResizeStart = React.useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      event.preventDefault();
      cleanupRef.current?.();
      const startX = event.clientX;
      const startWidth = widthPx;
      const previousCursor = document.body.style.cursor;
      const previousUserSelect = document.body.style.userSelect;
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";

      const cleanup = () => {
        document.body.style.cursor = previousCursor;
        document.body.style.userSelect = previousUserSelect;
        window.removeEventListener("pointermove", handlePointerMove);
        window.removeEventListener("pointerup", cleanup);
        cleanupRef.current = null;
      };
      const handlePointerMove = (moveEvent: PointerEvent) => {
        setWidthPx(
          clampAuxiliaryPanelWidth(
            startWidth + startX - moveEvent.clientX,
            window.innerWidth,
          ),
        );
      };
      cleanupRef.current = cleanup;
      window.addEventListener("pointermove", handlePointerMove);
      window.addEventListener("pointerup", cleanup, { once: true });
    },
    [widthPx],
  );

  return {
    canReset: widthPx !== PROJECT_CONTEXT_INSPECTOR_WIDTH_PX,
    onResetWidth: () => setWidthPx(PROJECT_CONTEXT_INSPECTOR_WIDTH_PX),
    onResizeStart,
    widthPx,
  };
}

/** Responsive, read-only Inspector driven exclusively by route selection. */
export function ProjectContextInspector({
  onAnnouncement,
  onClose,
  onFocusSelection,
  onOpenDocument,
  onOpenMeeting,
  onOpenProjectView,
  onSelect,
  onShowIncident,
  projectViewResult,
  result,
  selection,
  semanticRelationDocumentIds,
  semanticRootRelationDocumentIds,
  showIncidentDisabled = false,
}: {
  onAnnouncement?: (event: ProjectContextWorkspaceAnnouncementEvent) => void;
  onClose: () => void;
  onFocusSelection: () => void;
  onOpenDocument: (documentId: string) => void;
  onOpenMeeting: (meetingId: string) => void;
  onOpenProjectView: (objectId: string) => void;
  onSelect: (selection: ProjectContextRouteSelection) => void;
  onShowIncident: (coordinate: ProjectContextCoordinate) => void;
  projectViewResult?: ProjectViewLoadResult;
  result: ProjectContextQueryResult;
  selection: ProjectContextRouteSelection;
  semanticRelationDocumentIds?: ReadonlySet<string>;
  semanticRootRelationDocumentIds?: ReadonlySet<string>;
  showIncidentDisabled?: boolean;
}) {
  useEscapeKey(onClose);
  const inspectorWidth = useProjectContextInspectorWidth();
  const title = selection.kind === "edge" ? "Context Edge" : "Coordinate";

  return (
    <AuxiliaryPanel
      canResetWidth={inspectorWidth.canReset}
      className="z-30"
      header={
        <AuxiliaryPanelHeader bordered>
          <AuxiliaryPanelHeaderGroup>
            <AuxiliaryPanelHeaderTitleBlock
              subtitle={selection.key}
              subtitleTitle={selection.key}
              title={title}
            />
          </AuxiliaryPanelHeaderGroup>
          <AuxiliaryPanelHeaderActions>
            <Button
              aria-label={`Focus selected ${title}`}
              data-testid="project-context-focus-selection"
              onClick={onFocusSelection}
              size="icon"
              type="button"
              variant="ghost"
            >
              <LocateFixed />
            </Button>
          </AuxiliaryPanelHeaderActions>
        </AuxiliaryPanelHeader>
      }
      onClose={onClose}
      onResetWidth={inspectorWidth.onResetWidth}
      onResizeStart={inspectorWidth.onResizeStart}
      resizeHandleAriaLabel="Resize Project Context Inspector"
      resizeHandleTestId="project-context-inspector-resize-handle"
      splitPaneClamp={false}
      testId="project-context-inspector"
      widthPx={inspectorWidth.widthPx}
    >
      <AuxiliaryPanelBody className="overflow-y-auto" panelPadding>
        <ProjectContextInspectorContent
          onAnnouncement={onAnnouncement}
          onOpenDocument={onOpenDocument}
          onOpenMeeting={onOpenMeeting}
          onOpenProjectView={onOpenProjectView}
          onSelect={onSelect}
          onShowIncident={onShowIncident}
          projectViewResult={projectViewResult}
          result={result}
          selection={selection}
          semanticRelationDocumentIds={semanticRelationDocumentIds}
          semanticRootRelationDocumentIds={semanticRootRelationDocumentIds}
          showIncidentDisabled={showIncidentDisabled}
        />
      </AuxiliaryPanelBody>
    </AuxiliaryPanel>
  );
}
