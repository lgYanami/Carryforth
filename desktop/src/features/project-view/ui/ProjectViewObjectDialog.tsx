import { LoaderCircle, Plus, Save } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

import {
  identityFromMeta,
  useProjectDocumentMeta,
  useProjectDocumentMutation,
  useProjectDocuments,
} from "@/features/project-documents/hooks";
import { useProjectViewMutation } from "@/features/project-view/hooks";
import {
  indexProjectViewObjects,
  projectViewObjectPaths,
  projectViewObjectTypeLabel,
  type ProjectViewCreateContext,
  writableProjectViewObject,
} from "@/features/project-view/model";
import {
  PROJECT_VIEW_SELECT_CLASS,
  ProjectViewField,
} from "@/features/project-view/ui/ProjectViewFormFields";
import { ProjectViewObjectConflict } from "@/features/project-view/ui/ProjectViewObjectConflict";
import {
  ProjectViewObjectLifecycleFields,
  ProjectViewObjectRelationFields,
  ProjectViewObjectSummaryField,
  ProjectViewObjectTextFields,
} from "@/features/project-view/ui/ProjectViewObjectDialogFields";
import { CREATE_TYPES } from "@/features/project-view/ui/projectViewObjectFormOptions";
import {
  CREATE_GUIDE_VALUE,
  type ProjectViewObjectFormState as FormState,
} from "@/features/project-view/ui/projectViewObjectDialogState";
import type {
  ProjectView,
  ProjectViewIssueStatus,
  ProjectViewMutationResult,
  ProjectViewObject,
  ProjectViewObjectRef,
  ProjectViewObjectType,
  ProjectViewPlanStatus,
  ProjectViewRequirementStatus,
  ProjectViewStageStatus,
  ProjectViewWorkStatus,
  ProjectViewWritableObject,
} from "@/shared/api/tauriProjectView";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";

type CreatableObjectType = (typeof CREATE_TYPES)[number];

const BASE_FORM: FormState = {
  name: "",
  title: "",
  positioning: "",
  purpose: "",
  problem: "",
  scope: "",
  description: "",
  desiredOutcome: "",
  directions: "",
  responsibilities: "",
  boundaries: "",
  roleLevel: "member",
  active: true,
  status: "",
  priority: "normal",
  underGoalId: "",
  underPlanId: "",
  plannedInStageId: "",
  aboutId: "",
  handlesId: "",
  resourceKind: "repository",
  summary: "",
  guideDocumentId: "",
  guideTitle: "",
  guideSummary: "",
  guideContentMarkdown: "",
};

function defaultStatus(type: ProjectViewObjectType) {
  switch (type) {
    case "plan":
      return "draft";
    case "stage":
      return "planned";
    case "requirement":
      return "proposed";
    case "issue":
      return "open";
    case "work":
      return "pending";
    default:
      return "";
  }
}

function emptyForm(
  type: ProjectViewObjectType,
  context?: ProjectViewCreateContext,
): FormState {
  return {
    ...BASE_FORM,
    status: defaultStatus(type),
    underGoalId: context?.underGoalId ?? "",
    underPlanId: context?.underPlanId ?? "",
    plannedInStageId: context?.plannedInStageId ?? "",
    handlesId: context?.handles?.objectId ?? "",
  };
}

function formFromObject(object: ProjectViewObject): FormState {
  const form = emptyForm(object.objectType);
  switch (object.objectType) {
    case "project_profile":
      return { ...form, ...object.data, summary: object.data.summary ?? "" };
    case "goal":
      return {
        ...form,
        title: object.data.title,
        summary: object.data.summary ?? "",
        desiredOutcome: object.data.desiredOutcome,
        directions: object.data.directions.join("\n"),
      };
    case "role":
      return {
        ...form,
        name: object.data.name,
        summary: object.data.summary ?? "",
        purpose: object.data.purpose,
        responsibilities: object.data.responsibilities.join("\n"),
        boundaries: object.data.boundaries.join("\n"),
        active: object.data.active,
      };
    case "plan":
      return {
        ...form,
        title: object.data.title,
        summary: object.data.summary ?? "",
        description: object.data.description,
        status: object.data.status,
        underGoalId: object.relations.underGoalId ?? "",
      };
    case "stage":
      return {
        ...form,
        title: object.data.title,
        summary: object.data.summary ?? "",
        description: object.data.description,
        status: object.data.status,
        underPlanId: object.relations.underPlanId ?? "",
      };
    case "requirement":
      return {
        ...form,
        title: object.data.title,
        summary: object.data.summary ?? "",
        description: object.data.description,
        status: object.data.status,
        priority: object.data.priority,
        plannedInStageId: object.relations.plannedInStageId ?? "",
      };
    case "issue":
      return {
        ...form,
        title: object.data.title,
        summary: object.data.summary ?? "",
        description: object.data.description,
        status: object.data.status,
        priority: object.data.priority,
        plannedInStageId: object.relations.plannedInStageId ?? "",
        aboutId: object.relations.about?.objectId ?? "",
      };
    case "work":
      return {
        ...form,
        title: object.data.title,
        summary: object.data.summary ?? "",
        description: object.data.description,
        status: object.data.status,
        priority: object.data.priority,
        handlesId: object.relations.handles?.objectId ?? "",
      };
    case "resource":
      return {
        ...form,
        name: object.data.name,
        resourceKind: object.data.resourceKind,
        summary: object.data.summary ?? "",
        guideDocumentId: object.data.guideDocumentId,
      };
  }
}

function lines(value: string) {
  return value
    .split("\n")
    .map((item) => item.trim())
    .filter(Boolean);
}

function required(value: string, label: string) {
  const normalized = value.trim();
  if (!normalized) throw new Error(`${label} is required.`);
  return normalized;
}

function referenceFor(
  objectId: string,
  objects: ReadonlyMap<string, ProjectViewObject>,
  label: string,
): ProjectViewObjectRef {
  const object = objects.get(objectId);
  if (!object) throw new Error(`${label} must reference an active object.`);
  return { objectId: object.id, objectType: object.objectType };
}

function writableFromForm(
  type: ProjectViewObjectType,
  form: FormState,
  objects: ReadonlyMap<string, ProjectViewObject>,
  createdGuideDocumentId?: string,
): ProjectViewWritableObject {
  switch (type) {
    case "project_profile":
      return {
        objectType: type,
        data: {
          name: required(form.name, "Project name"),
          summary: form.summary.trim() || undefined,
          positioning: required(form.positioning, "Positioning"),
          purpose: required(form.purpose, "Purpose"),
          problem: required(form.problem, "Problem"),
          scope: required(form.scope, "Scope"),
        },
      };
    case "goal":
      return {
        objectType: type,
        data: {
          title: required(form.title, "Title"),
          summary: form.summary.trim() || undefined,
          desiredOutcome: required(form.desiredOutcome, "Desired outcome"),
          directions: lines(form.directions),
        },
      };
    case "role":
      return {
        objectType: type,
        data: {
          name: required(form.name, "Name"),
          summary: form.summary.trim() || undefined,
          purpose: required(form.purpose, "Purpose"),
          responsibilities: lines(form.responsibilities),
          boundaries: lines(form.boundaries),
          active: form.active,
        },
      };
    case "plan":
      return {
        objectType: type,
        data: {
          title: required(form.title, "Title"),
          summary: form.summary.trim() || undefined,
          description: required(form.description, "Description"),
          status: form.status as ProjectViewPlanStatus,
        },
        underGoalId: form.underGoalId || undefined,
      };
    case "stage":
      return {
        objectType: type,
        data: {
          title: required(form.title, "Title"),
          summary: form.summary.trim() || undefined,
          description: required(form.description, "Description"),
          status: form.status as ProjectViewStageStatus,
        },
        underPlanId: required(form.underPlanId, "Parent Plan"),
      };
    case "requirement":
      return {
        objectType: type,
        data: {
          title: required(form.title, "Title"),
          summary: form.summary.trim() || undefined,
          description: required(form.description, "Description"),
          status: form.status as ProjectViewRequirementStatus,
          priority: form.priority,
        },
        plannedInStageId: form.plannedInStageId || undefined,
      };
    case "issue":
      return {
        objectType: type,
        data: {
          title: required(form.title, "Title"),
          summary: form.summary.trim() || undefined,
          description: required(form.description, "Description"),
          status: form.status as ProjectViewIssueStatus,
          priority: form.priority,
        },
        plannedInStageId: form.plannedInStageId || undefined,
        about: form.aboutId
          ? referenceFor(form.aboutId, objects, "About")
          : undefined,
      };
    case "work":
      return {
        objectType: type,
        data: {
          title: required(form.title, "Title"),
          summary: form.summary.trim() || undefined,
          description: required(form.description, "Description"),
          status: form.status as ProjectViewWorkStatus,
          priority: form.priority,
        },
        handles: referenceFor(form.handlesId, objects, "Handles"),
      };
    case "resource":
      return {
        objectType: type,
        data: {
          name: required(form.name, "Name"),
          resourceKind: required(form.resourceKind, "Resource kind"),
          summary: form.summary.trim() || undefined,
          guideDocumentId: required(
            createdGuideDocumentId ?? form.guideDocumentId,
            "Guide",
          ),
        },
      };
  }
}

export function ProjectViewObjectDialog({
  canCreateAdminRole,
  canCreateRole,
  canGovernRole,
  context,
  initialType,
  mode,
  object,
  onApplied,
  onOpenChange,
  onReviewLatest,
  open,
  projectRevision,
  roleHasActiveAssignment,
  roleHasOpenProposal,
  roleActingAssignmentId,
  view,
}: {
  canCreateAdminRole: boolean;
  canCreateRole: boolean;
  canGovernRole: boolean;
  context?: ProjectViewCreateContext;
  initialType?: CreatableObjectType;
  mode: "create" | "edit";
  object?: ProjectViewObject;
  onApplied: (objectId?: string) => void;
  onOpenChange: (open: boolean) => void;
  onReviewLatest: () => Promise<unknown>;
  open: boolean;
  projectRevision: number;
  roleHasActiveAssignment?: boolean;
  roleHasOpenProposal?: boolean;
  roleActingAssignmentId?: string;
  view: ProjectView;
}) {
  const createTypes = React.useMemo(
    () => CREATE_TYPES.filter((type) => type !== "role" || canCreateRole),
    [canCreateRole],
  );
  const initialObjectType =
    mode === "edit" && object
      ? object.objectType
      : initialType === "role" && !canCreateRole
        ? "goal"
        : (initialType ?? "goal");
  const [objectType, setObjectType] =
    React.useState<ProjectViewObjectType>(initialObjectType);
  const mutation = useProjectViewMutation();
  const guideDocumentsEnabled = open && objectType === "resource";
  const documentMeta = useProjectDocumentMeta(guideDocumentsEnabled);
  const documents = useProjectDocuments(
    guideDocumentsEnabled ? documentMeta.data : undefined,
  );
  const documentMutation = useProjectDocumentMutation();
  const objects = React.useMemo(() => indexProjectViewObjects(view), [view]);
  const paths = React.useMemo(() => projectViewObjectPaths(view), [view]);
  const [form, setForm] = React.useState<FormState>(() =>
    object ? formFromObject(object) : emptyForm(initialObjectType, context),
  );
  const [baseRevision, setBaseRevision] = React.useState(projectRevision);
  const [summaryDirty, setSummaryDirty] = React.useState(false);
  const [error, setError] = React.useState<string>();
  const [rebasedRevision, setRebasedRevision] = React.useState<number>();
  const [reviewingLatest, setReviewingLatest] = React.useState(false);
  const [conflict, setConflict] = React.useState<
    Extract<ProjectViewMutationResult, { status: "conflict" }> | undefined
  >();
  const wasOpen = React.useRef(false);
  const resetMutation = mutation.reset;
  const resetDocumentMutation = documentMutation.reset;
  const guideOptions = React.useMemo(() => {
    const options = [
      {
        value: "",
        label: documents.isPending
          ? "Loading active Guides…"
          : documents.isError
            ? "Guides unavailable"
            : "Select an active Guide",
      },
      ...(documents.data?.documents.map((document) => ({
        value: document.documentId,
        label: `${document.title} · r${document.documentRevision}`,
      })) ?? []),
    ];
    if (
      form.guideDocumentId &&
      form.guideDocumentId !== CREATE_GUIDE_VALUE &&
      !options.some((option) => option.value === form.guideDocumentId)
    ) {
      options.push({
        value: form.guideDocumentId,
        label: `Current Guide · ${form.guideDocumentId}`,
      });
    }
    options.push({ value: CREATE_GUIDE_VALUE, label: "Create a new Guide…" });
    return options;
  }, [
    documents.data?.documents,
    documents.isError,
    documents.isPending,
    form.guideDocumentId,
  ]);

  React.useEffect(() => {
    if (open && !wasOpen.current) {
      const type =
        mode === "edit" && object
          ? object.objectType
          : initialType === "role" && !canCreateRole
            ? "goal"
            : (initialType ?? "goal");
      setObjectType(type);
      setForm(object ? formFromObject(object) : emptyForm(type, context));
      setBaseRevision(projectRevision);
      setSummaryDirty(false);
      setError(undefined);
      setRebasedRevision(undefined);
      setReviewingLatest(false);
      setConflict(undefined);
      resetMutation();
      resetDocumentMutation();
    }
    wasOpen.current = open;
  }, [
    context,
    canCreateRole,
    initialType,
    mode,
    object,
    open,
    projectRevision,
    resetMutation,
    resetDocumentMutation,
  ]);

  const set = React.useCallback(
    <K extends keyof FormState>(field: K, value: FormState[K]) => {
      if (field === "summary") setSummaryDirty(true);
      setForm((current) => ({ ...current, [field]: value }));
    },
    [],
  );

  const changeType = (type: CreatableObjectType) => {
    setObjectType(type);
    setForm(emptyForm(type));
    setSummaryDirty(false);
    setError(undefined);
  };

  const reviewLatest = async () => {
    if (reviewingLatest) return;
    setReviewingLatest(true);
    try {
      if (objectType === "role") {
        if (mode === "create" && !canCreateRole) {
          throw new Error(
            "Only the Community owner or an active Leader can create Roles.",
          );
        }
        if (mode === "edit" && !canGovernRole) {
          throw new Error(
            "Your current Community and Assignment state cannot govern this Role.",
          );
        }
        if (
          mode === "create" &&
          form.roleLevel === "admin" &&
          !canCreateAdminRole
        ) {
          throw new Error("Only the Community owner can create an admin Role.");
        }
      }
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
    if (mutation.isPending || documentMutation.isPending) return;
    setError(undefined);
    setRebasedRevision(undefined);
    setConflict(undefined);
    let createdGuideForRetry: string | undefined;
    try {
      if (
        (roleHasActiveAssignment || roleHasOpenProposal) &&
        objectType === "role" &&
        !form.active
      ) {
        setError(
          roleHasActiveAssignment
            ? "End or replace the active Assignment before deactivating this Role."
            : "Resolve or withdraw the open Proposal before deactivating this Role.",
        );
        return;
      }
      if (
        objectType === "resource" &&
        form.guideDocumentId === CREATE_GUIDE_VALUE
      ) {
        const meta = documentMeta.data;
        if (!meta) {
          throw new Error(
            "Project Documents must be available before creating a Resource Guide.",
          );
        }
        const guideResult = await documentMutation.mutateAsync({
          identity: identityFromMeta(meta),
          mutation: {
            type: "create",
            title: required(form.guideTitle, "Guide title"),
            summary: form.guideSummary.trim() || undefined,
            contentMarkdown: required(
              form.guideContentMarkdown,
              "Guide Markdown",
            ),
          },
        });
        if (guideResult.status === "conflict") {
          throw new Error("The Guide could not be created due to a conflict.");
        }
        createdGuideForRetry = guideResult.documentId;
        set("guideDocumentId", createdGuideForRetry);
      }
      const writable = writableFromForm(
        objectType,
        form,
        objects,
        createdGuideForRetry,
      );
      if (
        mode === "edit" &&
        object &&
        JSON.stringify(writable) ===
          JSON.stringify(writableProjectViewObject(object))
      ) {
        setError("Make at least one change before saving.");
        return;
      }
      const result = await mutation.mutateAsync(
        mode === "edit" && object
          ? {
              operation: "update",
              expectedProjectRevision: baseRevision,
              objectId: object.id,
              object: writable,
              summaryPatch: summaryDirty
                ? form.summary.trim() || null
                : undefined,
              actingAssignmentId:
                objectType === "role" ? roleActingAssignmentId : undefined,
            }
          : {
              operation: "create",
              expectedProjectRevision: baseRevision,
              object: writable as Exclude<
                ProjectViewWritableObject,
                { objectType: "project_profile" }
              >,
              initialRoleLevel:
                objectType === "role" ? form.roleLevel : undefined,
              actingAssignmentId:
                objectType === "role" ? roleActingAssignmentId : undefined,
            },
      );
      if (result.status === "conflict") {
        if (createdGuideForRetry) {
          toast.info(
            "Guide created and preserved; review the latest View, then retry the Resource.",
          );
        }
        setConflict(result);
        void reviewLatest();
        return;
      }
      if (result.confirmation === "superseded") {
        toast.warning(
          `${projectViewObjectTypeLabel(objectType)} changed again after this write; review the current object.`,
        );
      } else {
        toast.success(
          mode === "edit"
            ? `${projectViewObjectTypeLabel(objectType)} updated`
            : `${projectViewObjectTypeLabel(objectType)} created`,
        );
      }
      onOpenChange(false);
      onApplied(result.objectId);
    } catch (caught) {
      setError(
        `${
          caught instanceof Error
            ? caught.message
            : "The Project View change could not be submitted."
        }${
          createdGuideForRetry
            ? ` Guide ${createdGuideForRetry} was created and preserved for retry.`
            : ""
        }`,
      );
    }
  };

  return (
    <Dialog
      onOpenChange={(nextOpen) => {
        if (
          !mutation.isPending &&
          !documentMutation.isPending &&
          (nextOpen || !conflict)
        ) {
          onOpenChange(nextOpen);
        }
      }}
      open={open}
    >
      <DialogContent
        className="max-h-[calc(100vh-2rem)] overflow-y-auto sm:max-w-2xl"
        data-testid="project-view-object-dialog"
      >
        <form
          className="contents"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              {mode === "create" ? (
                <Plus className="h-4 w-4" />
              ) : (
                <Save className="h-4 w-4" />
              )}
              {mode === "create"
                ? "Add to View"
                : `Edit ${projectViewObjectTypeLabel(objectType)}`}
            </DialogTitle>
            <DialogDescription>
              This revision-checked change will be signed by your current Buzz
              identity. Base revision: {baseRevision}.
            </DialogDescription>
          </DialogHeader>

          <ProjectViewObjectConflict
            baseObject={object}
            conflict={conflict}
            latestObjects={objects}
            latestProjectRevision={projectRevision}
            mode={mode}
            onDiscardDraft={discardDraft}
            onReviewLatest={() => void reviewLatest()}
            onUseLatestRevision={useLatestRevision}
            rebasedRevision={rebasedRevision}
            reviewingLatest={reviewingLatest}
          />

          {mode === "create" && !initialType ? (
            <ProjectViewField label="Object type" required>
              <select
                className={PROJECT_VIEW_SELECT_CLASS}
                onChange={(event) =>
                  changeType(event.target.value as CreatableObjectType)
                }
                value={objectType}
              >
                {createTypes.map((type) => (
                  <option key={type} value={type}>
                    {projectViewObjectTypeLabel(type)}
                  </option>
                ))}
              </select>
            </ProjectViewField>
          ) : null}

          <div className="space-y-4">
            <ProjectViewObjectTextFields
              canCreateAdminRole={canCreateAdminRole}
              form={form}
              guideOptions={guideOptions}
              roleHasActiveAssignment={roleHasActiveAssignment}
              roleHasOpenProposal={roleHasOpenProposal}
              roleCreation={mode === "create"}
              set={set}
              type={objectType}
            />
            <ProjectViewObjectSummaryField form={form} set={set} />
            <ProjectViewObjectLifecycleFields
              form={form}
              set={set}
              type={objectType}
            />
            <ProjectViewObjectRelationFields
              editingId={object?.id}
              form={form}
              objects={objects}
              paths={paths}
              set={set}
              type={objectType}
            />
          </div>

          {error ? (
            <div
              className="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
              role="alert"
            >
              {error}
            </div>
          ) : null}

          <DialogFooter>
            <Button
              disabled={mutation.isPending || documentMutation.isPending}
              onClick={conflict ? discardDraft : () => onOpenChange(false)}
              type="button"
              variant="outline"
            >
              {conflict ? "Discard draft" : "Cancel"}
            </Button>
            <Button
              disabled={
                mutation.isPending ||
                documentMutation.isPending ||
                Boolean(conflict)
              }
              type="submit"
            >
              {mutation.isPending || documentMutation.isPending ? (
                <LoaderCircle className="animate-spin" />
              ) : mode === "create" ? (
                <Plus />
              ) : (
                <Save />
              )}
              {mutation.isPending || documentMutation.isPending
                ? "Submitting…"
                : mode === "create"
                  ? `Create ${projectViewObjectTypeLabel(objectType)}`
                  : "Save changes"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
