import { LocateFixed } from "lucide-react";

import { projectContextInspectedCoordinate } from "@/features/project-context/inspectorModel";
import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import { ProjectContextCoordinateInspector } from "@/features/project-context/ui/ProjectContextCoordinateInspector";
import { ProjectContextEdgeInspector } from "@/features/project-context/ui/ProjectContextEdgeInspector";
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
} from "@/shared/layout/AuxiliaryPanel";
import { Button } from "@/shared/ui/button";

/** Responsive, read-only Inspector driven exclusively by route selection. */
export function ProjectContextInspector({
  onClose,
  onFocusSelection,
  onOpenDocument,
  onOpenProjectView,
  onSelect,
  onShowIncident,
  projectViewResult,
  result,
  selection,
}: {
  onClose: () => void;
  onFocusSelection: () => void;
  onOpenDocument: (documentId: string) => void;
  onOpenProjectView: (objectId: string) => void;
  onSelect: (selection: ProjectContextRouteSelection) => void;
  onShowIncident: (coordinate: ProjectContextCoordinate) => void;
  projectViewResult?: ProjectViewLoadResult;
  result: ProjectContextQueryResult;
  selection: ProjectContextRouteSelection;
}) {
  useEscapeKey(onClose);
  const title = selection.kind === "edge" ? "Context Edge" : "Coordinate";

  return (
    <AuxiliaryPanel
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
      splitPaneClamp={false}
      testId="project-context-inspector"
      widthPx={440}
    >
      <AuxiliaryPanelBody className="overflow-y-auto" panelPadding>
        {selection.kind === "coordinate" ? (
          <ProjectContextCoordinateInspector
            detail={projectContextInspectedCoordinate(result, selection.key)}
            onOpenDocument={onOpenDocument}
            onOpenProjectView={onOpenProjectView}
            onSelectEdge={(edgeKey) => onSelect({ kind: "edge", key: edgeKey })}
            onShowIncident={onShowIncident}
            projectViewResult={projectViewResult}
            result={result}
          />
        ) : (
          <ProjectContextEdgeInspector
            edgeKey={selection.key}
            onOpenDocument={onOpenDocument}
            onSelectCoordinate={(key) => onSelect({ kind: "coordinate", key })}
            result={result}
          />
        )}
      </AuxiliaryPanelBody>
    </AuxiliaryPanel>
  );
}
