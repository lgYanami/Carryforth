import { LocateFixed } from "lucide-react";

import type { ProjectContextCoordinateOption } from "@/features/project-context/queryModel";
import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import type { ProjectContextSemanticOverlay } from "@/features/project-context/semanticOverlay";
import type {
  SemanticAttempt,
  SemanticSession,
} from "@/features/project-context/semanticSession";
import type { SemanticQueryDraft } from "@/features/project-context/semanticQueryModel";
import { ProjectContextInspectorContent } from "@/features/project-context/ui/ProjectContextInspectorContent";
import {
  type ProjectContextPickerSourceState,
  ProjectContextQueryBar,
} from "@/features/project-context/ui/ProjectContextQueryBar";
import { ProjectContextSemanticQueryBar } from "@/features/project-context/ui/ProjectContextSemanticQueryBar";
import { ProjectContextStructureOverview } from "@/features/project-context/ui/ProjectContextStructureOverview";
import type { ProjectContextWorkspaceTool } from "@/features/project-context/workspacePanelModel";
import type { ProjectContextWorkspaceAnnouncementEvent } from "@/features/project-context/workspacePanelModel";
import type {
  ProjectContextCoordinate,
  ProjectContextQuery,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import type { ProjectViewLoadResult } from "@/shared/api/tauriProjectView";
import { Button } from "@/shared/ui/button";

type PickerStates = {
  documents: ProjectContextPickerSourceState;
  meetings: ProjectContextPickerSourceState;
  projectView: ProjectContextPickerSourceState;
};

/** One active Structure, Semantic, or canonical Details pane. */
export function ProjectContextWorkspacePane({
  activeSemantic,
  appliedQuery,
  coordinateOptions,
  onApplyQuery,
  onAnnouncement,
  onCancelSemantic,
  onFitSemantic,
  onFocusSelection,
  onOpenDocument,
  onOpenMeeting,
  onOpenProjectView,
  onRunSemantic,
  onSelectionChange,
  onSemanticDraftChange,
  onStructuralDraftChange,
  pickerStates,
  projectViewResult,
  result,
  selection,
  semanticAttempt,
  semanticAvailable,
  semanticDraft,
  semanticFreshness,
  semanticNeedsAllContext,
  semanticOverlay,
  semanticTopologyAdvanced,
  structuralDraft,
  tool,
}: {
  activeSemantic: SemanticSession<ProjectContextSemanticOverlay> | null;
  appliedQuery: ProjectContextQuery;
  coordinateOptions: ProjectContextCoordinateOption[];
  onApplyQuery: (query: ProjectContextQuery) => void;
  onAnnouncement?: (event: ProjectContextWorkspaceAnnouncementEvent) => void;
  onCancelSemantic: () => void;
  onFitSemantic: () => void;
  onFocusSelection: () => void;
  onOpenDocument: (documentId: string) => void;
  onOpenMeeting: (meetingId: string) => void;
  onOpenProjectView: (objectId: string) => void;
  onRunSemantic: () => void;
  onSelectionChange: (selection: ProjectContextRouteSelection) => void;
  onSemanticDraftChange: (draft: SemanticQueryDraft) => void;
  onStructuralDraftChange: Parameters<
    typeof ProjectContextQueryBar
  >[0]["onDraftChange"];
  pickerStates: PickerStates;
  projectViewResult?: ProjectViewLoadResult;
  result?: ProjectContextQueryResult;
  selection: ProjectContextRouteSelection | null;
  semanticAttempt: SemanticAttempt;
  semanticAvailable: boolean;
  semanticDraft: SemanticQueryDraft;
  semanticFreshness: "snapshot" | "stale";
  semanticNeedsAllContext: boolean;
  semanticOverlay: ProjectContextSemanticOverlay | null;
  semanticTopologyAdvanced: boolean;
  structuralDraft: Parameters<typeof ProjectContextQueryBar>[0]["draft"];
  tool: ProjectContextWorkspaceTool;
}) {
  if (tool === "structure") {
    return (
      <>
        <ProjectContextStructureOverview
          appliedQuery={appliedQuery}
          displayedForSemanticResult={activeSemantic !== null}
          result={result}
        />
        <ProjectContextQueryBar
          appliedQuery={appliedQuery}
          coordinateOptions={coordinateOptions}
          documentsState={pickerStates.documents}
          draft={structuralDraft}
          meetingsState={pickerStates.meetings}
          onDraftChange={onStructuralDraftChange}
          onRun={onApplyQuery}
          panel
          projectViewState={pickerStates.projectView}
          runDisabled={semanticNeedsAllContext}
          runDisabledReason="Clear the semantic result before changing the structural query."
        />
      </>
    );
  }

  if (tool === "semantic") {
    return (
      <ProjectContextSemanticQueryBar
        active={activeSemantic}
        attempt={semanticAttempt}
        available={semanticAvailable}
        canFit={Boolean(semanticOverlay?.boundsTargetIds.length)}
        coordinateOptions={coordinateOptions}
        documentsState={pickerStates.documents}
        draft={semanticDraft}
        freshness={semanticFreshness}
        meetingsState={pickerStates.meetings}
        onCancel={onCancelSemantic}
        onDraftChange={onSemanticDraftChange}
        onFit={onFitSemantic}
        onRun={onRunSemantic}
        overlayVisible={semanticOverlay !== null}
        panel
        projectViewState={pickerStates.projectView}
        topologyAdvanced={semanticTopologyAdvanced}
      />
    );
  }

  if (!selection || !result) {
    return (
      <p className="text-sm text-muted-foreground">
        Select a Coordinate or Context Edge to inspect canonical details.
      </p>
    );
  }

  return (
    <div className="space-y-4">
      <Button
        className="w-full justify-start"
        data-testid="project-context-focus-selection"
        onClick={onFocusSelection}
        size="sm"
        type="button"
        variant="outline"
      >
        <LocateFixed />
        Focus selection
      </Button>
      <ProjectContextInspectorContent
        onAnnouncement={onAnnouncement}
        onOpenDocument={onOpenDocument}
        onOpenMeeting={onOpenMeeting}
        onOpenProjectView={onOpenProjectView}
        onSelect={onSelectionChange}
        onShowIncident={(coordinate: ProjectContextCoordinate) =>
          onApplyQuery({ type: "incident", coordinate })
        }
        projectViewResult={projectViewResult}
        result={result}
        selection={selection}
        semanticRelationDocumentIds={
          selection.kind === "edge"
            ? semanticOverlay?.relationDocumentIdsByEdge.get(selection.key)
            : undefined
        }
        semanticRootRelationDocumentIds={
          selection.kind === "edge"
            ? semanticOverlay?.rootRelationDocumentIdsByEdge.get(selection.key)
            : undefined
        }
        showIncidentDisabled={semanticNeedsAllContext}
      />
    </div>
  );
}
