import type {
  ProjectViewExplorerModel,
  ProjectViewExplorerSelection,
} from "@/features/project-view/explorerModel";
import { ProjectViewOutline } from "@/features/project-view/ui/ProjectViewOutline";
import { useIsAuxiliaryPanelOverlay } from "@/shared/hooks/use-mobile";
import {
  AuxiliaryPanel,
  AuxiliaryPanelBody,
  AuxiliaryPanelHeader,
  AuxiliaryPanelHeaderGroup,
  AuxiliaryPanelHeaderTitleBlock,
} from "@/shared/layout/AuxiliaryPanel";

const PROJECT_OUTLINE_WIDTH_PX = 320;

/** Collapsible right-side shell for the Project Outline tree. */
export function ProjectViewOutlinePanel({
  currentOccurrenceKey,
  model,
  onClose,
  onNavigate,
}: {
  currentOccurrenceKey: string;
  model: ProjectViewExplorerModel;
  onClose: () => void;
  onNavigate: (selection: ProjectViewExplorerSelection) => void;
}) {
  const isOverlay = useIsAuxiliaryPanelOverlay();
  return (
    <AuxiliaryPanel
      className="z-30 bg-background"
      header={
        <AuxiliaryPanelHeader bordered density="compact">
          <AuxiliaryPanelHeaderGroup>
            <AuxiliaryPanelHeaderTitleBlock title="Project Outline" />
          </AuxiliaryPanelHeaderGroup>
        </AuxiliaryPanelHeader>
      }
      onClose={onClose}
      testId="project-view-outline"
      widthPx={PROJECT_OUTLINE_WIDTH_PX}
    >
      <AuxiliaryPanelBody className="overflow-y-auto" panelPadding>
        <ProjectViewOutline
          currentOccurrenceKey={currentOccurrenceKey}
          model={model}
          onEscape={isOverlay ? onClose : undefined}
          onNavigate={onNavigate}
        />
      </AuxiliaryPanelBody>
    </AuxiliaryPanel>
  );
}
