import { AlertTriangle } from "lucide-react";
import * as React from "react";

import { formatProjectViewTerm } from "@/features/project-view/model";
import type { ProjectViewMutationResult } from "@/shared/api/tauriProjectView";
import { Button } from "@/shared/ui/button";
import { Textarea } from "@/shared/ui/textarea";

export const PROJECT_VIEW_SELECT_CLASS =
  "flex h-9 w-full rounded-lg border border-input/40 bg-background px-3 py-1 text-sm transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50";

export function ProjectViewField({
  children,
  description,
  label,
  required,
}: {
  children: React.ReactElement<{ id?: string }>;
  description?: string;
  label: string;
  required?: boolean;
}) {
  const generatedId = React.useId();
  const controlId = children.props.id ?? generatedId;
  return (
    <label className="block space-y-1.5" htmlFor={controlId}>
      <span className="text-sm font-medium">
        {label}
        {required ? <span className="ml-1 text-destructive">*</span> : null}
      </span>
      {React.cloneElement(children, { id: controlId })}
      {description ? (
        <span className="block text-xs leading-relaxed text-muted-foreground">
          {description}
        </span>
      ) : null}
    </label>
  );
}

export function ProjectViewSelect({
  label,
  onChange,
  options,
  required,
  value,
}: {
  label: string;
  onChange: (value: string) => void;
  options: Array<{ label: string; value: string }>;
  required?: boolean;
  value: string;
}) {
  return (
    <ProjectViewField label={label} required={required}>
      <select
        className={PROJECT_VIEW_SELECT_CLASS}
        onChange={(event) => onChange(event.target.value)}
        required={required}
        value={value}
      >
        {required && !options.some((option) => option.value === "") ? (
          <option disabled value="">
            Select an object
          </option>
        ) : null}
        {options.map((option) => (
          <option key={option.value || "none"} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
    </ProjectViewField>
  );
}

export function ProjectViewEnumSelect({
  label,
  onChange,
  values,
  value,
}: {
  label: string;
  onChange: (value: string) => void;
  values: readonly string[];
  value: string;
}) {
  return (
    <ProjectViewSelect
      label={label}
      onChange={onChange}
      options={values.map((item) => ({
        label: formatProjectViewTerm(item),
        value: item,
      }))}
      required
      value={value}
    />
  );
}

export function ProjectViewListField({
  label,
  onChange,
  value,
}: {
  label: string;
  onChange: (value: string) => void;
  value: string;
}) {
  return (
    <ProjectViewField
      description="Enter one item per line. Empty lines are ignored."
      label={label}
    >
      <Textarea
        className="min-h-24 resize-y"
        onChange={(event) => onChange(event.target.value)}
        value={value}
      />
    </ProjectViewField>
  );
}

export function ProjectViewConflictNotice({
  conflict,
  onReviewLatest,
}: {
  conflict: Extract<ProjectViewMutationResult, { status: "conflict" }>;
  onReviewLatest: () => void;
}) {
  const revision =
    conflict.currentProjectRevision === undefined
      ? "a newer revision"
      : `revision ${conflict.currentProjectRevision}`;
  return (
    <div
      className="rounded-xl border border-amber-500/40 bg-amber-500/10 p-3"
      role="alert"
    >
      <div className="flex gap-2">
        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-amber-600 dark:text-amber-400" />
        <div className="min-w-0">
          <div className="text-sm font-semibold">Project changed</div>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
            Your draft was based on revision {conflict.expectedProjectRevision},
            but the Project View is now at {revision}. Nothing was written and
            your input is still here.
          </p>
          <Button
            className="mt-2"
            onClick={onReviewLatest}
            size="sm"
            type="button"
            variant="outline"
          >
            Review latest View
          </Button>
        </div>
      </div>
    </div>
  );
}
