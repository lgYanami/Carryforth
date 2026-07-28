import { LoaderCircle, Plus, Save } from "lucide-react";
import * as React from "react";
import { toast } from "sonner";

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
  ProjectViewConflictNotice,
  ProjectViewEnumSelect,
  ProjectViewField,
  ProjectViewListField,
  ProjectViewSelect,
} from "@/features/project-view/ui/ProjectViewFormFields";
import type {
  ProjectView,
  ProjectViewIssueStatus,
  ProjectViewLocatorType,
  ProjectViewMutationResult,
  ProjectViewObject,
  ProjectViewObjectRef,
  ProjectViewObjectType,
  ProjectViewPlanStatus,
  ProjectViewPriority,
  ProjectViewRequirementStatus,
  ProjectViewResourceType,
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
import { Input } from "@/shared/ui/input";
import { Switch } from "@/shared/ui/switch";
import { Textarea } from "@/shared/ui/textarea";

const CREATE_TYPES = [
  "goal",
  "plan",
  "stage",
  "requirement",
  "issue",
  "work",
  "role",
  "resource",
] as const;

const PLAN_STATUSES = [
  "draft",
  "active",
  "paused",
  "completed",
  "cancelled",
] as const;
const STAGE_STATUSES = [
  "planned",
  "active",
  "paused",
  "completed",
  "cancelled",
] as const;
const REQUIREMENT_STATUSES = [
  "proposed",
  "ready",
  "in_progress",
  "satisfied",
  "withdrawn",
] as const;
const ISSUE_STATUSES = ["open", "in_progress", "resolved", "closed"] as const;
const WORK_STATUSES = [
  "pending",
  "in_progress",
  "paused",
  "submitted",
  "completed",
  "cancelled",
] as const;
const PRIORITIES = ["low", "normal", "high", "urgent"] as const;
const RESOURCE_TYPES = [
  "repository",
  "document",
  "design",
  "service",
  "environment",
  "artifact",
  "url",
] as const;
const LOCATOR_TYPES = [
  "url",
  "nostr_address",
  "nostr_event",
  "buzz_deep_link",
] as const;

type CreatableObjectType = (typeof CREATE_TYPES)[number];

type FormState = {
  name: string;
  title: string;
  positioning: string;
  purpose: string;
  problem: string;
  scope: string;
  description: string;
  desiredOutcome: string;
  directions: string;
  responsibilities: string;
  boundaries: string;
  active: boolean;
  status: string;
  priority: ProjectViewPriority;
  underGoalId: string;
  underPlanId: string;
  plannedInStageId: string;
  aboutId: string;
  handlesId: string;
  resourceType: ProjectViewResourceType;
  locatorType: ProjectViewLocatorType;
  locatorValue: string;
};

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
  active: true,
  status: "",
  priority: "normal",
  underGoalId: "",
  underPlanId: "",
  plannedInStageId: "",
  aboutId: "",
  handlesId: "",
  resourceType: "repository",
  locatorType: "url",
  locatorValue: "",
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
      return { ...form, ...object.data };
    case "goal":
      return {
        ...form,
        title: object.data.title,
        desiredOutcome: object.data.desiredOutcome,
        directions: object.data.directions.join("\n"),
      };
    case "role":
      return {
        ...form,
        name: object.data.name,
        purpose: object.data.purpose,
        responsibilities: object.data.responsibilities.join("\n"),
        boundaries: object.data.boundaries.join("\n"),
        active: object.data.active,
      };
    case "plan":
      return {
        ...form,
        title: object.data.title,
        description: object.data.description,
        status: object.data.status,
        underGoalId: object.relations.underGoalId ?? "",
      };
    case "stage":
      return {
        ...form,
        title: object.data.title,
        description: object.data.description,
        status: object.data.status,
        underPlanId: object.relations.underPlanId ?? "",
      };
    case "requirement":
      return {
        ...form,
        title: object.data.title,
        description: object.data.description,
        status: object.data.status,
        priority: object.data.priority,
        plannedInStageId: object.relations.plannedInStageId ?? "",
      };
    case "issue":
      return {
        ...form,
        title: object.data.title,
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
        description: object.data.description,
        status: object.data.status,
        priority: object.data.priority,
        handlesId: object.relations.handles?.objectId ?? "",
      };
    case "resource":
      return {
        ...form,
        name: object.data.name,
        description: object.data.description,
        resourceType: object.data.resourceType,
        locatorType: object.data.locator.locatorType,
        locatorValue: object.data.locator.value,
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
): ProjectViewWritableObject {
  switch (type) {
    case "project_profile":
      return {
        objectType: type,
        data: {
          name: required(form.name, "Project name"),
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
          desiredOutcome: required(form.desiredOutcome, "Desired outcome"),
          directions: lines(form.directions),
        },
      };
    case "role":
      return {
        objectType: type,
        data: {
          name: required(form.name, "Name"),
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
          resourceType: form.resourceType,
          locator: {
            locatorType: form.locatorType,
            value: required(form.locatorValue, "Locator"),
          },
          description: required(form.description, "Description"),
        },
      };
  }
}

function TextFields({
  form,
  set,
  type,
}: {
  form: FormState;
  set: <K extends keyof FormState>(field: K, value: FormState[K]) => void;
  type: ProjectViewObjectType;
}) {
  if (type === "project_profile") {
    return (
      <>
        <ProjectViewField label="Project name" required>
          <Input
            autoFocus
            onChange={(event) => set("name", event.target.value)}
            value={form.name}
          />
        </ProjectViewField>
        {[
          ["Positioning", "positioning"],
          ["Purpose", "purpose"],
          ["Problem", "problem"],
          ["Scope", "scope"],
        ].map(([label, field]) => (
          <ProjectViewField key={field} label={label} required>
            <Textarea
              onChange={(event) =>
                set(
                  field as "positioning" | "purpose" | "problem" | "scope",
                  event.target.value,
                )
              }
              value={
                form[field as "positioning" | "purpose" | "problem" | "scope"]
              }
            />
          </ProjectViewField>
        ))}
      </>
    );
  }
  if (type === "goal") {
    return (
      <>
        <ProjectViewField label="Title" required>
          <Input
            autoFocus
            onChange={(event) => set("title", event.target.value)}
            value={form.title}
          />
        </ProjectViewField>
        <ProjectViewField label="Desired outcome" required>
          <Textarea
            onChange={(event) => set("desiredOutcome", event.target.value)}
            value={form.desiredOutcome}
          />
        </ProjectViewField>
        <ProjectViewListField
          label="Directions"
          onChange={(value) => set("directions", value)}
          value={form.directions}
        />
      </>
    );
  }
  if (type === "role") {
    return (
      <>
        <ProjectViewField label="Name" required>
          <Input
            autoFocus
            onChange={(event) => set("name", event.target.value)}
            value={form.name}
          />
        </ProjectViewField>
        <ProjectViewField label="Purpose" required>
          <Textarea
            onChange={(event) => set("purpose", event.target.value)}
            value={form.purpose}
          />
        </ProjectViewField>
        <ProjectViewListField
          label="Responsibilities"
          onChange={(value) => set("responsibilities", value)}
          value={form.responsibilities}
        />
        <ProjectViewListField
          label="Boundaries"
          onChange={(value) => set("boundaries", value)}
          value={form.boundaries}
        />
        <div className="flex items-center justify-between rounded-lg border border-border/70 p-3">
          <div>
            <div className="text-sm font-medium">Active role</div>
            <div className="text-xs text-muted-foreground">
              This is semantic project state, not Buzz authorization.
            </div>
          </div>
          <Switch
            checked={form.active}
            onCheckedChange={(value) => set("active", value)}
          />
        </div>
      </>
    );
  }
  if (type === "resource") {
    return (
      <>
        <ProjectViewField label="Name" required>
          <Input
            autoFocus
            onChange={(event) => set("name", event.target.value)}
            value={form.name}
          />
        </ProjectViewField>
        <ProjectViewEnumSelect
          label="Resource type"
          onChange={(value) =>
            set("resourceType", value as ProjectViewResourceType)
          }
          value={form.resourceType}
          values={RESOURCE_TYPES}
        />
        <ProjectViewEnumSelect
          label="Locator type"
          onChange={(value) =>
            set("locatorType", value as ProjectViewLocatorType)
          }
          value={form.locatorType}
          values={LOCATOR_TYPES}
        />
        <ProjectViewField label="Locator" required>
          <Input
            onChange={(event) => set("locatorValue", event.target.value)}
            value={form.locatorValue}
          />
        </ProjectViewField>
        <ProjectViewField label="Description" required>
          <Textarea
            onChange={(event) => set("description", event.target.value)}
            value={form.description}
          />
        </ProjectViewField>
      </>
    );
  }
  return (
    <>
      <ProjectViewField label="Title" required>
        <Input
          autoFocus
          onChange={(event) => set("title", event.target.value)}
          value={form.title}
        />
      </ProjectViewField>
      <ProjectViewField label="Description" required>
        <Textarea
          onChange={(event) => set("description", event.target.value)}
          value={form.description}
        />
      </ProjectViewField>
    </>
  );
}

function LifecycleFields({
  form,
  set,
  type,
}: {
  form: FormState;
  set: <K extends keyof FormState>(field: K, value: FormState[K]) => void;
  type: ProjectViewObjectType;
}) {
  const statuses =
    type === "plan"
      ? PLAN_STATUSES
      : type === "stage"
        ? STAGE_STATUSES
        : type === "requirement"
          ? REQUIREMENT_STATUSES
          : type === "issue"
            ? ISSUE_STATUSES
            : type === "work"
              ? WORK_STATUSES
              : undefined;
  if (!statuses) return null;
  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <ProjectViewEnumSelect
        label="Status"
        onChange={(value) => set("status", value)}
        value={form.status}
        values={statuses}
      />
      {type === "requirement" || type === "issue" || type === "work" ? (
        <ProjectViewEnumSelect
          label="Priority"
          onChange={(value) => set("priority", value as ProjectViewPriority)}
          value={form.priority}
          values={PRIORITIES}
        />
      ) : null}
    </div>
  );
}

function RelationFields({
  editingId,
  form,
  paths,
  objects,
  set,
  type,
}: {
  editingId?: string;
  form: FormState;
  paths: ReadonlyMap<string, string>;
  objects: ReadonlyMap<string, ProjectViewObject>;
  set: <K extends keyof FormState>(field: K, value: FormState[K]) => void;
  type: ProjectViewObjectType;
}) {
  const options = (targetTypes: ProjectViewObjectType[], optional: boolean) => [
    ...(optional ? [{ value: "", label: "None" }] : []),
    ...Array.from(objects.values())
      .filter(
        (object) =>
          object.id !== editingId && targetTypes.includes(object.objectType),
      )
      .map((object) => ({
        value: object.id,
        label:
          paths.get(object.id) ?? projectViewObjectTypeLabel(object.objectType),
      }))
      .sort((left, right) => left.label.localeCompare(right.label)),
  ];
  switch (type) {
    case "plan":
      return (
        <ProjectViewSelect
          label="Goal"
          onChange={(value) => set("underGoalId", value)}
          options={options(["goal"], true)}
          value={form.underGoalId}
        />
      );
    case "stage":
      return (
        <ProjectViewSelect
          label="Parent Plan"
          onChange={(value) => set("underPlanId", value)}
          options={options(["plan"], false)}
          required
          value={form.underPlanId}
        />
      );
    case "requirement":
      return (
        <ProjectViewSelect
          label="Planned in Stage"
          onChange={(value) => set("plannedInStageId", value)}
          options={options(["stage"], true)}
          value={form.plannedInStageId}
        />
      );
    case "issue":
      return (
        <div className="grid gap-4 sm:grid-cols-2">
          <ProjectViewSelect
            label="Planned in Stage"
            onChange={(value) => set("plannedInStageId", value)}
            options={options(["stage"], true)}
            value={form.plannedInStageId}
          />
          <ProjectViewSelect
            label="About"
            onChange={(value) => set("aboutId", value)}
            options={options(
              [
                "project_profile",
                "goal",
                "role",
                "plan",
                "stage",
                "requirement",
                "issue",
                "work",
                "resource",
              ],
              true,
            )}
            value={form.aboutId}
          />
        </div>
      );
    case "work":
      return (
        <ProjectViewSelect
          label="Handles"
          onChange={(value) => set("handlesId", value)}
          options={options(["requirement", "issue"], false)}
          required
          value={form.handlesId}
        />
      );
    default:
      return null;
  }
}

export function ProjectViewObjectDialog({
  context,
  initialType,
  mode,
  object,
  onApplied,
  onOpenChange,
  onReviewLatest,
  open,
  projectRevision,
  view,
}: {
  context?: ProjectViewCreateContext;
  initialType?: CreatableObjectType;
  mode: "create" | "edit";
  object?: ProjectViewObject;
  onApplied: (objectId?: string) => void;
  onOpenChange: (open: boolean) => void;
  onReviewLatest: () => void;
  open: boolean;
  projectRevision: number;
  view: ProjectView;
}) {
  const mutation = useProjectViewMutation();
  const objects = React.useMemo(() => indexProjectViewObjects(view), [view]);
  const paths = React.useMemo(() => projectViewObjectPaths(view), [view]);
  const initialObjectType =
    mode === "edit" && object ? object.objectType : (initialType ?? "goal");
  const [objectType, setObjectType] =
    React.useState<ProjectViewObjectType>(initialObjectType);
  const [form, setForm] = React.useState<FormState>(() =>
    object ? formFromObject(object) : emptyForm(initialObjectType, context),
  );
  const [baseRevision, setBaseRevision] = React.useState(projectRevision);
  const [error, setError] = React.useState<string>();
  const [conflict, setConflict] = React.useState<
    Extract<ProjectViewMutationResult, { status: "conflict" }> | undefined
  >();
  const wasOpen = React.useRef(false);
  const resetMutation = mutation.reset;

  React.useEffect(() => {
    if (open && !wasOpen.current) {
      const type =
        mode === "edit" && object ? object.objectType : (initialType ?? "goal");
      setObjectType(type);
      setForm(object ? formFromObject(object) : emptyForm(type, context));
      setBaseRevision(projectRevision);
      setError(undefined);
      setConflict(undefined);
      resetMutation();
    }
    wasOpen.current = open;
  }, [
    context,
    initialType,
    mode,
    object,
    open,
    projectRevision,
    resetMutation,
  ]);

  const set = React.useCallback(
    <K extends keyof FormState>(field: K, value: FormState[K]) =>
      setForm((current) => ({ ...current, [field]: value })),
    [],
  );

  const changeType = (type: CreatableObjectType) => {
    setObjectType(type);
    setForm(emptyForm(type));
    setError(undefined);
  };

  const submit = async () => {
    if (mutation.isPending) return;
    setError(undefined);
    setConflict(undefined);
    try {
      const writable = writableFromForm(objectType, form, objects);
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
            }
          : {
              operation: "create",
              expectedProjectRevision: baseRevision,
              object: writable as Exclude<
                ProjectViewWritableObject,
                { objectType: "project_profile" }
              >,
            },
      );
      if (result.status === "conflict") {
        setConflict(result);
        return;
      }
      toast.success(
        mode === "edit"
          ? `${projectViewObjectTypeLabel(objectType)} updated`
          : `${projectViewObjectTypeLabel(objectType)} created`,
      );
      onOpenChange(false);
      onApplied(result.objectId);
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : "The Project View change could not be submitted.",
      );
    }
  };

  return (
    <Dialog
      onOpenChange={(nextOpen) => {
        if (!mutation.isPending) onOpenChange(nextOpen);
      }}
      open={open}
    >
      <DialogContent
        className="max-h-[calc(100vh-2rem)] overflow-y-auto sm:max-w-2xl"
        data-testid="project-view-object-dialog"
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

        {conflict ? (
          <ProjectViewConflictNotice
            conflict={conflict}
            onReviewLatest={() => {
              onOpenChange(false);
              onReviewLatest();
            }}
          />
        ) : null}

        {mode === "create" && !initialType ? (
          <ProjectViewField label="Object type" required>
            <select
              className={PROJECT_VIEW_SELECT_CLASS}
              onChange={(event) =>
                changeType(event.target.value as CreatableObjectType)
              }
              value={objectType}
            >
              {CREATE_TYPES.map((type) => (
                <option key={type} value={type}>
                  {projectViewObjectTypeLabel(type)}
                </option>
              ))}
            </select>
          </ProjectViewField>
        ) : null}

        <div className="space-y-4">
          <TextFields form={form} set={set} type={objectType} />
          <LifecycleFields form={form} set={set} type={objectType} />
          <RelationFields
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
            disabled={mutation.isPending}
            onClick={() => onOpenChange(false)}
            type="button"
            variant="outline"
          >
            Cancel
          </Button>
          <Button
            disabled={mutation.isPending}
            onClick={() => void submit()}
            type="button"
          >
            {mutation.isPending ? (
              <LoaderCircle className="animate-spin" />
            ) : mode === "create" ? (
              <Plus />
            ) : (
              <Save />
            )}
            {mutation.isPending
              ? "Submitting…"
              : mode === "create"
                ? `Create ${projectViewObjectTypeLabel(objectType)}`
                : "Save changes"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
