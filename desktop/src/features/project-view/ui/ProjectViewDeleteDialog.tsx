import { LoaderCircle, Trash2 } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import { useProjectViewMutation } from "@/features/project-view/hooks";
import {
  indexProjectViewObjects,
  projectViewIncomingReferences,
  projectViewObjectTitle,
  projectViewObjectTypeLabel,
} from "@/features/project-view/model";
import { ProjectViewConflictNotice } from "@/features/project-view/ui/ProjectViewFormFields";
import type {
  ProjectView,
  ProjectViewMutationResult,
  ProjectViewObject,
} from "@/shared/api/tauriProjectView";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Button } from "@/shared/ui/button";

export function ProjectViewDeleteDialog({
  actingAssignmentId,
  object,
  onDeleted,
  onOpenChange,
  onReviewLatest,
  open,
  projectRevision,
  view,
}: {
  actingAssignmentId?: string;
  object?: ProjectViewObject;
  onDeleted: () => void;
  onOpenChange: (open: boolean) => void;
  onReviewLatest: () => Promise<unknown>;
  open: boolean;
  projectRevision: number;
  view: ProjectView;
}) {
  const mutation = useProjectViewMutation();
  const [baseRevision, setBaseRevision] = React.useState(projectRevision);
  const [error, setError] = React.useState<string>();
  const [rebasedRevision, setRebasedRevision] = React.useState<number>();
  const [reviewingLatest, setReviewingLatest] = React.useState(false);
  const [conflict, setConflict] = React.useState<
    Extract<ProjectViewMutationResult, { status: "conflict" }> | undefined
  >();
  const wasOpen = React.useRef(false);
  const resetMutation = mutation.reset;

  React.useEffect(() => {
    if (open && !wasOpen.current) {
      setBaseRevision(projectRevision);
      setError(undefined);
      setRebasedRevision(undefined);
      setReviewingLatest(false);
      setConflict(undefined);
      resetMutation();
    }
    wasOpen.current = open;
  }, [open, projectRevision, resetMutation]);

  if (!object) return null;
  const objects = indexProjectViewObjects(view);
  const latestObject = objects.get(object.id);
  const incoming = projectViewIncomingReferences(view, object.id);
  const lastGoal = object.objectType === "goal" && view.goals.length === 1;
  const profile = object.objectType === "project_profile";
  const targetMissing = latestObject === undefined;
  const blocked = profile || lastGoal || targetMissing || incoming.length > 0;

  const reviewLatest = async () => {
    if (reviewingLatest) return;
    setReviewingLatest(true);
    try {
      await onReviewLatest();
    } finally {
      setReviewingLatest(false);
    }
  };

  const discardDraft = () => {
    setConflict(undefined);
    onOpenChange(false);
  };

  const useLatestRevision = () => {
    setBaseRevision(projectRevision);
    setConflict(undefined);
    setError(undefined);
    setRebasedRevision(projectRevision);
  };

  const submit = async () => {
    if (mutation.isPending || blocked) return;
    setError(undefined);
    setRebasedRevision(undefined);
    setConflict(undefined);
    try {
      const result = await mutation.mutateAsync({
        operation: "delete",
        expectedProjectRevision: baseRevision,
        objectType: object.objectType,
        objectId: object.id,
        actingAssignmentId:
          object.objectType === "role" ? actingAssignmentId : undefined,
      });
      if (result.status === "conflict") {
        setConflict(result);
        void reviewLatest();
        return;
      }
      if (result.confirmation === "superseded") {
        toast.warning(
          `${projectViewObjectTypeLabel(object.objectType)} changed again after deletion; review the current object.`,
        );
      } else {
        toast.success(
          `${projectViewObjectTypeLabel(object.objectType)} deleted`,
        );
      }
      onOpenChange(false);
      onDeleted();
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : "The Project View object could not be deleted.",
      );
    }
  };

  return (
    <AlertDialog
      onOpenChange={(nextOpen) => {
        if (!mutation.isPending && (nextOpen || !conflict)) {
          onOpenChange(nextOpen);
        }
      }}
      open={open}
    >
      <AlertDialogContent data-testid="project-view-delete-dialog">
        <AlertDialogHeader>
          <AlertDialogTitle>
            Delete {projectViewObjectTitle(object)}?
          </AlertDialogTitle>
          <AlertDialogDescription>
            Project View never cascades deletion. This action creates a
            permanent tombstone for the{" "}
            {projectViewObjectTypeLabel(object.objectType).toLowerCase()}.
          </AlertDialogDescription>
        </AlertDialogHeader>

        {conflict ? (
          <ProjectViewConflictNotice
            comparison={
              <p className="text-xs leading-relaxed text-muted-foreground">
                {latestObject
                  ? latestObject.objectRevision === object.objectRevision
                    ? `The target object is still at object revision ${object.objectRevision}; another project object changed.`
                    : `The target object changed from object revision ${object.objectRevision} to ${latestObject.objectRevision}. Re-check references before rebasing the delete.`
                  : "The target no longer exists in the latest verified View, so this delete cannot be rebased."}
              </p>
            }
            conflict={conflict}
            latestProjectRevision={projectRevision}
            onDiscardDraft={discardDraft}
            onReviewLatest={() => void reviewLatest()}
            onUseLatestRevision={latestObject ? useLatestRevision : undefined}
            refreshing={reviewingLatest}
          />
        ) : null}

        {rebasedRevision !== undefined ? (
          <div
            className="rounded-lg border border-blue-500/40 bg-blue-500/10 px-3 py-2 text-xs leading-relaxed text-muted-foreground"
            role="status"
          >
            Delete intent now uses verified project revision {rebasedRevision}.
            Review the target and references, then confirm deletion again.
          </div>
        ) : null}

        {profile ? (
          <div className="rounded-lg border border-border/70 bg-muted/30 p-3 text-sm">
            The Project Profile is permanent and can only be edited.
          </div>
        ) : null}
        {lastGoal ? (
          <div className="rounded-lg border border-border/70 bg-muted/30 p-3 text-sm">
            Every initialized View must retain at least one Goal.
          </div>
        ) : null}
        {targetMissing ? (
          <div className="rounded-lg border border-border/70 bg-muted/30 p-3 text-sm">
            This object is no longer active in the latest verified View.
          </div>
        ) : null}
        {incoming.length > 0 ? (
          <section className="rounded-lg border border-amber-500/40 bg-amber-500/10 p-3">
            <div className="text-sm font-semibold">
              Move or unlink these references first
            </div>
            <ul className="mt-2 space-y-1.5 text-xs text-muted-foreground">
              {incoming.map(({ relation, source }) => (
                <li key={`${source.id}-${relation}`}>
                  {projectViewObjectTypeLabel(source.objectType)} “
                  {projectViewObjectTitle(source)}” references this object
                  through {relation}.
                </li>
              ))}
            </ul>
          </section>
        ) : null}
        {error ? (
          <div
            className="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
            role="alert"
          >
            {error}
          </div>
        ) : null}

        <AlertDialogFooter>
          {conflict ? (
            <Button
              disabled={mutation.isPending}
              onClick={discardDraft}
              type="button"
              variant="outline"
            >
              Discard delete
            </Button>
          ) : (
            <AlertDialogCancel asChild>
              <Button
                disabled={mutation.isPending}
                type="button"
                variant="outline"
              >
                Cancel
              </Button>
            </AlertDialogCancel>
          )}
          <Button
            disabled={blocked || mutation.isPending}
            onClick={() => void submit()}
            type="button"
            variant="destructive"
          >
            {mutation.isPending ? (
              <LoaderCircle className="animate-spin" />
            ) : (
              <Trash2 />
            )}
            {mutation.isPending ? "Deleting…" : "Delete object"}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
