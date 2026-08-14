import { projectViewObjectTypeLabel } from "@/features/project-view/model";
import {
  ProjectViewEnumSelect,
  ProjectViewField,
  ProjectViewListField,
  ProjectViewSelect,
} from "@/features/project-view/ui/ProjectViewFormFields";
import {
  ISSUE_STATUSES,
  PLAN_STATUSES,
  PRIORITIES,
  REQUIREMENT_STATUSES,
  STAGE_STATUSES,
  WORK_STATUSES,
} from "@/features/project-view/ui/projectViewObjectFormOptions";
import {
  CREATE_GUIDE_VALUE,
  type ProjectViewObjectFormState,
} from "@/features/project-view/ui/projectViewObjectDialogState";
import type {
  ProjectRoleLevel,
  ProjectViewObject,
  ProjectViewObjectType,
  ProjectViewPriority,
} from "@/shared/api/tauriProjectView";
import { Input } from "@/shared/ui/input";
import { Switch } from "@/shared/ui/switch";
import { Textarea } from "@/shared/ui/textarea";

type SetFormField = <K extends keyof ProjectViewObjectFormState>(
  field: K,
  value: ProjectViewObjectFormState[K],
) => void;

export function ProjectViewObjectTextFields({
  canCreateAdminRole,
  form,
  guideOptions,
  roleHasActiveAssignment,
  roleHasOpenProposal,
  roleHasResponsibleWork,
  roleCreation,
  set,
  type,
}: {
  canCreateAdminRole: boolean;
  form: ProjectViewObjectFormState;
  guideOptions: Array<{ value: string; label: string }>;
  roleHasActiveAssignment?: boolean;
  roleHasOpenProposal?: boolean;
  roleHasResponsibleWork?: boolean;
  roleCreation: boolean;
  set: SetFormField;
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
        {roleCreation ? (
          <ProjectViewEnumSelect
            label="Role level"
            onChange={(value) => set("roleLevel", value as ProjectRoleLevel)}
            value={form.roleLevel}
            values={canCreateAdminRole ? ["member", "admin"] : ["member"]}
          />
        ) : null}
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
              {roleHasActiveAssignment
                ? "End or replace the active Assignment before deactivating this Role."
                : roleHasOpenProposal
                  ? "Resolve or withdraw the open Proposal before deactivating this Role."
                  : roleHasResponsibleWork
                    ? "Clear or reassign this Role's Work before deactivating this Role."
                    : "An active Assignment projects this Role's level into Community membership."}
            </div>
          </div>
          <Switch
            aria-label="Active role"
            checked={form.active}
            disabled={
              roleHasActiveAssignment ||
              roleHasOpenProposal ||
              roleHasResponsibleWork
            }
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
        <ProjectViewField label="Resource kind" required>
          <Input
            onChange={(event) => set("resourceKind", event.target.value)}
            placeholder="repository, service, design-system, …"
            value={form.resourceKind}
          />
        </ProjectViewField>
        <ProjectViewSelect
          label="Guide"
          onChange={(value) => set("guideDocumentId", value)}
          options={guideOptions}
          required
          value={form.guideDocumentId}
        />
        {form.guideDocumentId === CREATE_GUIDE_VALUE ? (
          <div className="space-y-4 rounded-lg border border-border/70 bg-muted/20 p-3">
            <div>
              <div className="text-sm font-medium">Create Guide first</div>
              <p className="mt-1 text-xs text-muted-foreground">
                The Guide is committed as an independent Document before the
                Resource. If the Resource conflicts, the Guide is preserved for
                retry.
              </p>
            </div>
            <ProjectViewField label="Guide title" required>
              <Input
                onChange={(event) => set("guideTitle", event.target.value)}
                value={form.guideTitle}
              />
            </ProjectViewField>
            <ProjectViewField label="Guide summary">
              <Input
                onChange={(event) => set("guideSummary", event.target.value)}
                value={form.guideSummary}
              />
            </ProjectViewField>
            <ProjectViewField label="Guide Markdown" required>
              <Textarea
                className="min-h-40 font-mono"
                onChange={(event) =>
                  set("guideContentMarkdown", event.target.value)
                }
                value={form.guideContentMarkdown}
              />
            </ProjectViewField>
          </div>
        ) : null}
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

export function ProjectViewObjectSummaryField({
  form,
  set,
}: {
  form: ProjectViewObjectFormState;
  set: SetFormField;
}) {
  return (
    <ProjectViewField label="Retrieval summary">
      <Textarea
        onChange={(event) => set("summary", event.target.value)}
        placeholder="What this object covers and when it is worth loading"
        value={form.summary}
      />
    </ProjectViewField>
  );
}

export function ProjectViewObjectLifecycleFields({
  form,
  set,
  type,
}: {
  form: ProjectViewObjectFormState;
  set: SetFormField;
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

export function ProjectViewObjectRelationFields({
  editingId,
  form,
  paths,
  objects,
  set,
  type,
}: {
  editingId?: string;
  form: ProjectViewObjectFormState;
  paths: ReadonlyMap<string, string>;
  objects: ReadonlyMap<string, ProjectViewObject>;
  set: SetFormField;
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
