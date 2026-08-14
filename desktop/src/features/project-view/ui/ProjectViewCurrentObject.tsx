import type * as React from "react";

import type {
  ProjectViewExplorerSelection,
  ProjectViewObjectPage,
} from "@/features/project-view/explorerModel";
import {
  formatProjectViewTerm,
  projectViewObjectPriority,
  projectViewObjectStatus,
  projectViewObjectTitle,
  projectViewObjectTypeLabel,
} from "@/features/project-view/model";
import { ProjectViewObjectDetails } from "@/features/project-view/ui/ProjectViewObjectDetails";
import { ProjectViewParentNavigation } from "@/features/project-view/ui/ProjectViewParentNavigation";
import { ProjectViewRelatedContextItems } from "@/features/project-view/ui/ProjectViewRelatedContextItems";
import { ProjectViewSummaryGroup } from "@/features/project-view/ui/ProjectViewSummaryGroup";
import type { ProjectViewSummaryEntry } from "@/features/project-view/ui/ProjectViewSummaryItem";
import { Badge } from "@/shared/ui/badge";
import { Markdown } from "@/shared/ui/markdown";

function selectionForSummary(
  item: ProjectViewSummaryEntry,
): ProjectViewExplorerSelection {
  return item.kind === "object"
    ? {
        kind: "object",
        objectId: item.objectId,
        via: item.occurrenceKey,
      }
    : {
        kind: "document",
        documentId: item.documentId,
        revision: item.documentRevision,
        via: item.occurrenceKey,
      };
}

/** Full current object plus summaries of exactly one direct Explorer layer. */
export function ProjectViewCurrentObject({
  actions,
  children,
  headingRef,
  onNavigate,
  page,
}: {
  actions?: React.ReactNode;
  children?: React.ReactNode;
  headingRef?: React.Ref<HTMLHeadingElement>;
  onNavigate: (selection: ProjectViewExplorerSelection) => void;
  page: ProjectViewObjectPage;
}) {
  const object = page.currentObject;
  const status = projectViewObjectStatus(object);
  const priority = projectViewObjectPriority(object);
  const selectSummary = (item: ProjectViewSummaryEntry) =>
    onNavigate(selectionForSummary(item));

  return (
    <article
      className="mx-auto w-full max-w-6xl space-y-7 px-5 py-6"
      data-object-id={object.id}
      data-testid="project-view-current-object"
    >
      <header className="space-y-4 border-b border-border/70 pb-6">
        <div className="flex min-w-0 items-start gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant="outline">
                {projectViewObjectTypeLabel(object.objectType)}
              </Badge>
              {status ? (
                <Badge variant="secondary">
                  {formatProjectViewTerm(status)}
                </Badge>
              ) : null}
              {priority ? (
                <Badge variant="outline">
                  {formatProjectViewTerm(priority)}
                </Badge>
              ) : null}
            </div>
            <h1
              className="mt-3 break-words text-2xl font-semibold tracking-tight outline-hidden"
              ref={headingRef}
              tabIndex={-1}
            >
              {projectViewObjectTitle(object)}
            </h1>
          </div>
          <ProjectViewParentNavigation
            onSelect={(parent) =>
              onNavigate({ kind: "object", objectId: parent.objectId })
            }
            parent={page.parent}
          />
        </div>

        <section
          className="rounded-xl border border-border/70 bg-muted/20 p-4"
          data-testid="project-view-current-summary"
        >
          <h2 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Summary
          </h2>
          {object.data.summary ? (
            <Markdown
              className="mt-2 text-sm leading-6"
              content={object.data.summary}
              interactive={false}
            />
          ) : (
            <p className="mt-2 text-sm italic text-muted-foreground">
              No summary provided.
            </p>
          )}
        </section>
        {actions ? <div>{actions}</div> : null}
      </header>

      <section
        className="grid gap-5 rounded-xl border border-border/70 bg-card/50 p-5 md:grid-cols-2"
        data-testid="project-view-current-details"
      >
        <ProjectViewObjectDetails
          object={object}
          showResourceGuideAction={false}
        />
      </section>

      {children}

      {page.structuralGroups.length > 0 ? (
        <section
          className="space-y-6 border-t border-border/70 pt-6"
          data-testid="project-view-direct-children"
        >
          {page.structuralGroups.map((group) => (
            <ProjectViewSummaryGroup
              items={group.items}
              key={group.label}
              label={group.label}
              onSelect={selectSummary}
            />
          ))}
        </section>
      ) : null}

      <ProjectViewRelatedContextItems
        documents={page.documents}
        onSelect={selectSummary}
        relatedIssues={page.relatedIssues}
        relatedResources={page.relatedResources}
      />
    </article>
  );
}
