import { projectContextInspectedCoordinate } from "@/features/project-context/inspectorModel";
import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import type { ProjectContextWorkspaceAnnouncementEvent } from "@/features/project-context/workspacePanelModel";
import { ProjectContextCoordinateInspector } from "@/features/project-context/ui/ProjectContextCoordinateInspector";
import { ProjectContextEdgeInspector } from "@/features/project-context/ui/ProjectContextEdgeInspector";
import type {
  ProjectContextCoordinate,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import type { ProjectViewLoadResult } from "@/shared/api/tauriProjectView";

/** Canonical, read-only Inspector content without an owning panel shell. */
export function ProjectContextInspectorContent({
  onAnnouncement,
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
  return selection.kind === "coordinate" ? (
    <ProjectContextCoordinateInspector
      detail={projectContextInspectedCoordinate(result, selection.key)}
      onAnnouncement={onAnnouncement}
      onOpenDocument={onOpenDocument}
      onOpenMeeting={onOpenMeeting}
      onOpenProjectView={onOpenProjectView}
      onSelectEdge={(edgeKey) => onSelect({ kind: "edge", key: edgeKey })}
      onShowIncident={onShowIncident}
      projectViewResult={projectViewResult}
      result={result}
      showIncidentDisabled={showIncidentDisabled}
    />
  ) : (
    <ProjectContextEdgeInspector
      edgeKey={selection.key}
      onOpenDocument={onOpenDocument}
      onSelectCoordinate={(key) => onSelect({ kind: "coordinate", key })}
      result={result}
      semanticRelationDocumentIds={semanticRelationDocumentIds}
      semanticRootRelationDocumentIds={semanticRootRelationDocumentIds}
    />
  );
}
