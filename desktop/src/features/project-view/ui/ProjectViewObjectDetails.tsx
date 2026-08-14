import { ArrowRight } from "lucide-react";
import { Link } from "@tanstack/react-router";
import type * as React from "react";

import { formatProjectViewTerm } from "@/features/project-view/model";
import type { ProjectViewObject } from "@/shared/api/tauriProjectView";
import { Button } from "@/shared/ui/button";

export function ProjectViewDetail({
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

/** Render all source-owned fields for the current Project View object. */
export function ProjectViewObjectDetails({
  object,
  showResourceGuideAction = true,
}: {
  object: ProjectViewObject;
  showResourceGuideAction?: boolean;
}) {
  switch (object.objectType) {
    case "project_profile":
      return (
        <>
          <ProjectViewDetail label="Positioning">
            {object.data.positioning}
          </ProjectViewDetail>
          <ProjectViewDetail label="Purpose">
            {object.data.purpose}
          </ProjectViewDetail>
          <ProjectViewDetail label="Problem">
            {object.data.problem}
          </ProjectViewDetail>
          <ProjectViewDetail label="Scope">
            {object.data.scope}
          </ProjectViewDetail>
        </>
      );
    case "goal":
      return (
        <>
          <ProjectViewDetail label="Desired outcome">
            {object.data.desiredOutcome}
          </ProjectViewDetail>
          <ProjectViewDetail label="Directions">
            <StringList items={object.data.directions} />
          </ProjectViewDetail>
        </>
      );
    case "role":
      return (
        <>
          <ProjectViewDetail label="Purpose">
            {object.data.purpose}
          </ProjectViewDetail>
          <ProjectViewDetail label="Responsibilities">
            <StringList items={object.data.responsibilities} />
          </ProjectViewDetail>
          <ProjectViewDetail label="Boundaries">
            <StringList items={object.data.boundaries} />
          </ProjectViewDetail>
        </>
      );
    case "resource":
      return (
        <>
          <ProjectViewDetail label="Resource kind">
            {formatProjectViewTerm(object.data.resourceKind)}
          </ProjectViewDetail>
          {showResourceGuideAction ? (
            <ProjectViewDetail label="Guide">
              <Button asChild size="sm" variant="outline">
                <Link
                  search={{ document: object.data.guideDocumentId }}
                  to="/documents"
                >
                  Open verified Guide
                  <ArrowRight />
                </Link>
              </Button>
            </ProjectViewDetail>
          ) : null}
        </>
      );
    case "plan":
    case "stage":
    case "requirement":
    case "issue":
    case "work":
      return (
        <ProjectViewDetail label="Description">
          {object.data.description}
        </ProjectViewDetail>
      );
  }
}
