import { projectViewObjectTitle } from "@/features/project-view/model";
import { ProjectViewContextSection } from "@/features/project-view/ui/ProjectViewContextSection";
import type { ProjectViewObject } from "@/shared/api/tauriProjectView";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";

/** On-demand mutation surface; Main keeps Context reading in summary items. */
export function ProjectViewContextManagementDialog({
  actingAssignmentId,
  canMutate,
  contextCapability,
  object,
  objectsById,
  onOpenChange,
  onRefresh,
  onSelectObject,
  open,
  projectRevision,
}: {
  actingAssignmentId?: string;
  canMutate: boolean;
  contextCapability: boolean;
  object?: ProjectViewObject;
  objectsById: ReadonlyMap<string, ProjectViewObject>;
  onOpenChange: (open: boolean) => void;
  onRefresh: () => Promise<unknown>;
  onSelectObject: (objectId: string) => void;
  open: boolean;
  projectRevision: number;
}) {
  if (!object) return null;
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent
        className="flex max-h-[85vh] flex-col sm:max-w-2xl"
        data-testid="project-view-context-dialog"
      >
        <DialogHeader>
          <DialogTitle>
            Manage Context for {projectViewObjectTitle(object)}
          </DialogTitle>
          <DialogDescription>
            Add or remove verified Resource and Document coordinates. Context
            does not change the Project hierarchy.
          </DialogDescription>
        </DialogHeader>
        <div className="min-h-0 overflow-y-auto pr-1">
          <ProjectViewContextSection
            actingAssignmentId={actingAssignmentId}
            canMutate={canMutate}
            contextCapability={contextCapability}
            object={object}
            objectsById={objectsById}
            onRefresh={onRefresh}
            onSelectObject={onSelectObject}
            projectRevision={projectRevision}
          />
        </div>
      </DialogContent>
    </Dialog>
  );
}
