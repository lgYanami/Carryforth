import { ListChevronsDownUp, ListChevronsUpDown } from "lucide-react";
import * as React from "react";

import type {
  ProjectViewExplorerModel,
  ProjectViewExplorerSelection,
} from "@/features/project-view/explorerModel";
import {
  ProjectViewOutline,
  type ProjectViewOutlineHandle,
} from "@/features/project-view/ui/ProjectViewOutline";
import { useIsAuxiliaryPanelOverlay } from "@/shared/hooks/use-mobile";
import {
  AuxiliaryPanel,
  AuxiliaryPanelBody,
  AuxiliaryPanelHeader,
  AuxiliaryPanelHeaderActions,
  AuxiliaryPanelHeaderGroup,
  AuxiliaryPanelHeaderTitleBlock,
} from "@/shared/layout/AuxiliaryPanel";
import { Button } from "@/shared/ui/button";

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
  const outlineRef = React.useRef<ProjectViewOutlineHandle>(null);
  return (
    <AuxiliaryPanel
      className="z-30 bg-background"
      header={
        <AuxiliaryPanelHeader bordered density="compact">
          <AuxiliaryPanelHeaderGroup>
            <AuxiliaryPanelHeaderTitleBlock title="Project Outline" />
          </AuxiliaryPanelHeaderGroup>
          <AuxiliaryPanelHeaderActions>
            <Button
              aria-label="Expand all Project Outline branches"
              data-testid="project-view-outline-expand-all"
              onClick={() => outlineRef.current?.expandAll()}
              size="icon"
              title="Expand all"
              type="button"
              variant="ghost"
            >
              <ListChevronsUpDown />
            </Button>
            <Button
              aria-label="Collapse all Project Outline branches"
              data-testid="project-view-outline-collapse-all"
              onClick={() => outlineRef.current?.collapseAll()}
              size="icon"
              title="Collapse all"
              type="button"
              variant="ghost"
            >
              <ListChevronsDownUp />
            </Button>
          </AuxiliaryPanelHeaderActions>
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
          ref={outlineRef}
        />
      </AuxiliaryPanelBody>
    </AuxiliaryPanel>
  );
}
