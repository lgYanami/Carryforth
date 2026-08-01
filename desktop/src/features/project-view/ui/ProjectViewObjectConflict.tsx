import { projectViewObjectTypeLabel } from "@/features/project-view/model";
import { ProjectViewConflictNotice } from "@/features/project-view/ui/ProjectViewFormFields";
import type {
  ProjectViewMutationResult,
  ProjectViewObject,
} from "@/shared/api/tauriProjectView";

export function ProjectViewObjectConflict({
  baseObject,
  conflict,
  latestObjects,
  latestProjectRevision,
  mode,
  onDiscardDraft,
  onReviewLatest,
  onUseLatestRevision,
  rebasedRevision,
  reviewingLatest,
}: {
  baseObject?: ProjectViewObject;
  conflict?: Extract<ProjectViewMutationResult, { status: "conflict" }>;
  latestObjects: ReadonlyMap<string, ProjectViewObject>;
  latestProjectRevision: number;
  mode: "create" | "edit";
  onDiscardDraft: () => void;
  onReviewLatest: () => void;
  onUseLatestRevision: () => void;
  rebasedRevision?: number;
  reviewingLatest: boolean;
}) {
  const latestObject = baseObject
    ? latestObjects.get(baseObject.id)
    : undefined;

  return (
    <>
      {conflict ? (
        <ProjectViewConflictNotice
          comparison={
            mode === "edit" && baseObject ? (
              <p className="text-xs leading-relaxed text-muted-foreground">
                {latestObject
                  ? latestObject.objectRevision === baseObject.objectRevision
                    ? `The target ${projectViewObjectTypeLabel(baseObject.objectType).toLowerCase()} is still at object revision ${baseObject.objectRevision}; another project object changed.`
                    : `The target ${projectViewObjectTypeLabel(baseObject.objectType).toLowerCase()} changed from object revision ${baseObject.objectRevision} to ${latestObject.objectRevision}. Review your full draft before rebasing.`
                  : `The target ${projectViewObjectTypeLabel(baseObject.objectType).toLowerCase()} no longer exists in the latest verified View. This draft cannot be rebased.`}
              </p>
            ) : (
              <p className="text-xs leading-relaxed text-muted-foreground">
                Another project change landed after this create draft began.
                Review the latest relations before choosing a new base.
              </p>
            )
          }
          conflict={conflict}
          latestProjectRevision={latestProjectRevision}
          onDiscardDraft={onDiscardDraft}
          onReviewLatest={onReviewLatest}
          onUseLatestRevision={
            mode === "edit" && baseObject && !latestObject
              ? undefined
              : onUseLatestRevision
          }
          refreshing={reviewingLatest}
        />
      ) : null}

      {rebasedRevision !== undefined ? (
        <div
          className="rounded-lg border border-blue-500/40 bg-blue-500/10 px-3 py-2 text-xs leading-relaxed text-muted-foreground"
          role="status"
        >
          Draft now uses verified project revision {rebasedRevision} as its
          base. Review every field and relation, then submit explicitly.
        </div>
      ) : null}
    </>
  );
}
