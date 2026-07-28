import {
  ArrowRight,
  Link2,
  Pencil,
  ShieldCheck,
  Trash2,
  X,
} from "lucide-react";

import {
  formatProjectViewTerm,
  projectViewObjectDescription,
  projectViewObjectPriority,
  projectViewObjectStatus,
  projectViewObjectTitle,
  projectViewObjectTypeLabel,
} from "@/features/project-view/model";
import type {
  ProjectView,
  ProjectViewObject,
} from "@/shared/api/tauriProjectView";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { PubKey } from "@/shared/ui/PubKey";

type ProjectViewInspectorProps = {
  object: ProjectViewObject;
  objectsById: ReadonlyMap<string, ProjectViewObject>;
  onClose: () => void;
  onDelete: (object: ProjectViewObject) => void;
  onEdit: (object: ProjectViewObject) => void;
  onSelectObject: (objectId: string) => void;
  view: ProjectView;
};

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function formatDateTime(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : dateTimeFormatter.format(date);
}

function Detail({
  children,
  label,
}: {
  children: React.ReactNode;
  label: string;
}) {
  return (
    <div>
      <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      <div className="mt-1 text-sm leading-relaxed">{children}</div>
    </div>
  );
}

function StringList({ items }: { items: string[] }) {
  if (items.length === 0) {
    return <span className="text-muted-foreground">None</span>;
  }
  return (
    <ul className="space-y-1.5">
      {items.map((item) => (
        <li className="flex gap-2" key={item}>
          <span className="mt-2 h-1 w-1 shrink-0 rounded-full bg-muted-foreground" />
          <span>{item}</span>
        </li>
      ))}
    </ul>
  );
}

function ObjectDetails({ object }: { object: ProjectViewObject }) {
  switch (object.objectType) {
    case "project_profile":
      return (
        <>
          <Detail label="Positioning">{object.data.positioning}</Detail>
          <Detail label="Purpose">{object.data.purpose}</Detail>
          <Detail label="Problem">{object.data.problem}</Detail>
          <Detail label="Scope">{object.data.scope}</Detail>
        </>
      );
    case "goal":
      return (
        <>
          <Detail label="Desired outcome">{object.data.desiredOutcome}</Detail>
          <Detail label="Directions">
            <StringList items={object.data.directions} />
          </Detail>
        </>
      );
    case "role":
      return (
        <>
          <Detail label="Purpose">{object.data.purpose}</Detail>
          <Detail label="Responsibilities">
            <StringList items={object.data.responsibilities} />
          </Detail>
          <Detail label="Boundaries">
            <StringList items={object.data.boundaries} />
          </Detail>
        </>
      );
    case "resource":
      return (
        <>
          <Detail label="Description">{object.data.description}</Detail>
          <Detail label="Resource type">
            {formatProjectViewTerm(object.data.resourceType)}
          </Detail>
          <Detail
            label={formatProjectViewTerm(object.data.locator.locatorType)}
          >
            <span className="break-all font-mono text-xs">
              {object.data.locator.value}
            </span>
          </Detail>
        </>
      );
    case "plan":
    case "stage":
    case "requirement":
    case "issue":
    case "work":
      return <Detail label="Description">{object.data.description}</Detail>;
  }
}

function RelationLink({
  label,
  objectId,
  objectsById,
  onSelectObject,
}: {
  label: string;
  objectId: string;
  objectsById: ReadonlyMap<string, ProjectViewObject>;
  onSelectObject: (objectId: string) => void;
}) {
  const target = objectsById.get(objectId);
  return (
    <button
      className="flex w-full items-center gap-2 rounded-lg border border-border/70 bg-muted/20 px-2.5 py-2 text-left transition-colors hover:bg-muted/50 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
      onClick={() => onSelectObject(objectId)}
      type="button"
    >
      <Link2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1">
        <span className="block text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          {label}
        </span>
        <span className="block truncate text-xs font-medium">
          {target ? projectViewObjectTitle(target) : objectId}
        </span>
      </span>
      <ArrowRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
    </button>
  );
}

export function ProjectViewInspector({
  object,
  objectsById,
  onClose,
  onDelete,
  onEdit,
  onSelectObject,
  view,
}: ProjectViewInspectorProps) {
  const status = projectViewObjectStatus(object);
  const priority = projectViewObjectPriority(object);
  const issueRefs = view.issueReferencesByTarget[object.id] ?? [];
  const relations = [
    object.relations.underGoalId
      ? { label: "Under goal", id: object.relations.underGoalId }
      : null,
    object.relations.underPlanId
      ? { label: "Under plan", id: object.relations.underPlanId }
      : null,
    object.relations.plannedInStageId
      ? { label: "Planned in stage", id: object.relations.plannedInStageId }
      : null,
    object.relations.about
      ? { label: "About", id: object.relations.about.objectId }
      : null,
    object.relations.handles
      ? { label: "Handles", id: object.relations.handles.objectId }
      : null,
  ].filter((relation): relation is { label: string; id: string } =>
    Boolean(relation),
  );

  return (
    <aside
      className="absolute inset-y-0 right-0 z-30 flex w-full max-w-sm shrink-0 flex-col border-l border-border bg-background shadow-xl lg:static lg:w-96 lg:shadow-none"
      data-testid="project-view-inspector"
    >
      <div className="flex items-start gap-3 border-b border-border/70 p-4">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <Badge variant="outline">
              {projectViewObjectTypeLabel(object.objectType)}
            </Badge>
            {status ? (
              <Badge variant="secondary">{formatProjectViewTerm(status)}</Badge>
            ) : null}
            {priority ? (
              <Badge variant="outline">{formatProjectViewTerm(priority)}</Badge>
            ) : null}
          </div>
          <h2 className="mt-2 text-lg font-semibold leading-tight">
            {projectViewObjectTitle(object)}
          </h2>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
            {projectViewObjectDescription(object)}
          </p>
        </div>
        <Button
          aria-label="Close inspector"
          onClick={onClose}
          size="icon"
          type="button"
          variant="ghost"
        >
          <X />
        </Button>
      </div>

      <div className="min-h-0 flex-1 space-y-5 overflow-y-auto p-4">
        <section className="grid grid-cols-2 gap-2">
          <Button
            onClick={() => onEdit(object)}
            type="button"
            variant="outline"
          >
            <Pencil />
            Edit
          </Button>
          <Button
            onClick={() => onDelete(object)}
            type="button"
            variant="outline"
          >
            <Trash2 />
            Delete
          </Button>
        </section>

        <div className="space-y-4">
          <ObjectDetails object={object} />
        </div>

        {relations.length > 0 || issueRefs.length > 0 ? (
          <section className="space-y-2">
            <h3 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Relations
            </h3>
            {relations.map((relation) => (
              <RelationLink
                key={`${relation.label}-${relation.id}`}
                label={relation.label}
                objectId={relation.id}
                objectsById={objectsById}
                onSelectObject={onSelectObject}
              />
            ))}
            {issueRefs.map((reference) => (
              <RelationLink
                key={`issue-${reference.objectId}`}
                label="Related issue"
                objectId={reference.objectId}
                objectsById={objectsById}
                onSelectObject={onSelectObject}
              />
            ))}
          </section>
        ) : null}

        <section className="space-y-3 border-t border-border/70 pt-4">
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-emerald-600 dark:text-emerald-400" />
            <h3 className="text-xs font-semibold">Verified projection</h3>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Detail label="Object revision">{object.objectRevision}</Detail>
            <Detail label="Project revision">{object.projectRevision}</Detail>
          </div>
          <Detail label="Created">
            <span>{formatDateTime(object.createdAt)}</span>
            <span className="mt-1 block text-xs text-muted-foreground">
              by{" "}
              <PubKey
                className="text-xs"
                pubkey={object.createdBy}
                testId="project-view-created-by"
              />
            </span>
          </Detail>
          <Detail label="Last updated">
            <span>{formatDateTime(object.updatedAt)}</span>
            <span className="mt-1 block text-xs text-muted-foreground">
              by{" "}
              <PubKey
                className="text-xs"
                pubkey={object.updatedBy}
                testId="project-view-updated-by"
              />
            </span>
          </Detail>
          <Detail label="Object ID">
            <span className="break-all font-mono text-xs">{object.id}</span>
          </Detail>
        </section>
      </div>
    </aside>
  );
}
