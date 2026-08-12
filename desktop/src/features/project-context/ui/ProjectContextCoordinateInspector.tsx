import {
  Archive,
  ArrowRight,
  ArrowUpRight,
  CloudOff,
  Network,
  ShieldCheck,
} from "lucide-react";
import type { ReactNode } from "react";

import {
  projectContextDocumentIdentity,
  projectContextIncidentEdgeKeys,
  projectContextProjectViewObject,
  projectContextProjectViewRelations,
} from "@/features/project-context/inspectorModel";
import { ProjectContextDocumentContent } from "@/features/project-context/ui/ProjectContextDocumentContent";
import { ProjectContextMeetingContent } from "@/features/project-context/ui/ProjectContextMeetingContent";
import {
  formatProjectViewTerm,
  projectViewObjectDescription,
  projectViewObjectPriority,
  projectViewObjectStatus,
  projectViewObjectTitle,
  projectViewObjectTypeLabel,
} from "@/features/project-view/model";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import type {
  ProjectViewLoadResult,
  ProjectViewObject,
} from "@/shared/api/tauriProjectView";
import type {
  ProjectContextCoordinate,
  ProjectContextCoordinateDetail,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import type { ProjectContextWorkspaceAnnouncementEvent } from "@/features/project-context/workspacePanelModel";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Markdown } from "@/shared/ui/markdown";

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function formatDateTime(value?: string) {
  if (!value) return "Unknown";
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : dateTimeFormatter.format(date);
}

function Detail({ children, label }: { children: ReactNode; label: string }) {
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
  return items.length > 0 ? (
    <ul className="space-y-1.5">
      {items.map((item) => (
        <li className="flex gap-2" key={item}>
          <span className="mt-2 h-1 w-1 shrink-0 rounded-full bg-muted-foreground" />
          <span>{item}</span>
        </li>
      ))}
    </ul>
  ) : (
    <span className="text-muted-foreground">None</span>
  );
}

function ProjectViewBody({ object }: { object: ProjectViewObject }) {
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
          <Detail label="Resource kind">
            {formatProjectViewTerm(object.data.resourceKind)}
          </Detail>
          <Detail label="Guide Document ID">
            <code className="break-all text-xs">
              {object.data.guideDocumentId}
            </code>
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

function ProjectViewContent({
  object,
  onOpenProjectView,
  projectViewResult,
}: {
  object: ProjectViewObject;
  onOpenProjectView: (objectId: string) => void;
  projectViewResult: Extract<ProjectViewLoadResult, { status: "ready" }>;
}) {
  const status = projectViewObjectStatus(object);
  const priority = projectViewObjectPriority(object);
  const relations = projectContextProjectViewRelations(
    projectViewResult.view,
    object,
  );
  const description =
    object.objectType === "resource"
      ? formatProjectViewTerm(object.data.resourceKind)
      : projectViewObjectDescription(object);
  return (
    <div
      className="space-y-5"
      data-testid="project-context-project-view-detail"
    >
      <section>
        <div className="flex flex-wrap items-center gap-2">
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
        <h3 className="mt-2 text-lg font-semibold leading-tight">
          {projectViewObjectTitle(object)}
        </h3>
        <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
          {description}
        </p>
        {object.data.summary ? (
          <section
            className="mt-3 rounded-lg border border-border/70 bg-muted/20 p-3"
            data-testid="project-context-project-view-summary"
          >
            <h4 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Summary
            </h4>
            <Markdown
              className="mt-2 text-sm leading-6"
              content={object.data.summary}
              interactive={false}
            />
          </section>
        ) : null}
        <Button
          className="mt-3"
          data-testid="project-context-open-project-view"
          onClick={() => onOpenProjectView(object.id)}
          size="sm"
          type="button"
          variant="outline"
        >
          Open in Project View
          <ArrowUpRight />
        </Button>
      </section>

      <section className="space-y-4 rounded-xl border border-border/70 bg-muted/20 p-3">
        <ProjectViewBody object={object} />
      </section>

      {relations.length > 0 ? (
        <section className="space-y-2">
          <h3 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Direct relations
          </h3>
          {relations.map((relation) => (
            <div
              className="flex items-center gap-2 rounded-lg border border-border/70 bg-muted/20 px-3 py-2"
              key={`${relation.direction}-${relation.label}-${relation.target.id}`}
            >
              <span className="min-w-0 flex-1">
                <span className="block text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                  {relation.direction === "incoming"
                    ? `${formatProjectViewTerm(relation.label)} from`
                    : relation.label}
                </span>
                <span className="block truncate text-sm font-medium">
                  {projectViewObjectTitle(relation.target)}
                </span>
              </span>
              <Badge variant="outline">
                {projectViewObjectTypeLabel(relation.target.objectType)}
              </Badge>
            </div>
          ))}
        </section>
      ) : null}

      <section className="space-y-3 border-t border-border/70 pt-4">
        <div className="flex items-center gap-2">
          <ShieldCheck className="h-4 w-4 text-emerald-600 dark:text-emerald-400" />
          <h3 className="text-xs font-semibold">Verified Project View</h3>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <Detail label="Object revision">{object.objectRevision}</Detail>
          <Detail label="Project revision">{object.projectRevision}</Detail>
        </div>
        <Detail label="Last updated">
          <span>{formatDateTime(object.updatedAt)}</span>
          <span className="mt-1 block text-xs text-muted-foreground">
            by <ProjectViewActor compact pubkey={object.updatedBy} />
          </span>
        </Detail>
      </section>
    </div>
  );
}

function stableCoordinateId(coordinate: ProjectContextCoordinate) {
  if (coordinate.type === "document") return coordinate.documentId;
  if (coordinate.type === "meeting") return coordinate.meetingId;
  return coordinate.objectId;
}

/** Read-only detail for a selected real Coordinate. */
export function ProjectContextCoordinateInspector({
  detail,
  onOpenDocument,
  onOpenMeeting,
  onOpenProjectView,
  onAnnouncement,
  onSelectEdge,
  onShowIncident,
  showIncidentDisabled = false,
  projectViewResult,
  result,
}: {
  detail: ProjectContextCoordinateDetail;
  onOpenDocument: (documentId: string) => void;
  onOpenMeeting: (meetingId: string) => void;
  onOpenProjectView: (objectId: string) => void;
  onAnnouncement?: (event: ProjectContextWorkspaceAnnouncementEvent) => void;
  onSelectEdge: (edgeKey: string) => void;
  onShowIncident: (coordinate: ProjectContextCoordinate) => void;
  showIncidentDisabled?: boolean;
  projectViewResult?: ProjectViewLoadResult;
  result: ProjectContextQueryResult;
}) {
  const coordinate = detail.coordinate;
  const stableId = stableCoordinateId(coordinate);
  const edgeKeys = projectContextIncidentEdgeKeys(result, detail.coordinateKey);
  const object = projectContextProjectViewObject({
    detail,
    projectViewResult,
    result,
  });
  const matchingView =
    object && projectViewResult?.status === "ready"
      ? projectViewResult
      : undefined;
  const typeLabel =
    coordinate.type === "document"
      ? "Document"
      : coordinate.type === "meeting"
        ? "Meeting"
        : projectViewObjectTypeLabel(coordinate.objectType);
  const documentDetail =
    coordinate.type === "document"
      ? {
          documentId: coordinate.documentId,
          state: detail.state,
          title: detail.title,
          documentRevision: detail.documentRevision,
          updatedAt: detail.updatedAt,
          updatedBy: detail.updatedBy,
          unavailableReason: detail.unavailableReason,
        }
      : undefined;

  return (
    <div
      className="space-y-5 p-4"
      data-testid="project-context-coordinate-inspector"
    >
      <section>
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline">{typeLabel} Coordinate</Badge>
          {detail.state === "active" ? (
            <Badge variant="success">Active</Badge>
          ) : detail.state === "terminal" ? (
            <Badge variant="success">Terminal</Badge>
          ) : detail.state === "tombstoned" ? (
            <Badge variant="secondary">
              <Archive className="mr-1 h-3 w-3" />
              Tombstoned
            </Badge>
          ) : (
            <Badge variant="warning">
              <CloudOff className="mr-1 h-3 w-3" />
              Unavailable
            </Badge>
          )}
        </div>
        <Button
          className="mt-3 w-full"
          data-testid="project-context-show-incident"
          disabled={showIncidentDisabled}
          onClick={() => onShowIncident(coordinate)}
          size="sm"
          type="button"
          variant="outline"
        >
          <Network />
          Show incident Context
        </Button>
        {showIncidentDisabled ? (
          <p className="mt-2 text-xs text-muted-foreground">
            Clear the semantic result before changing the structural query.
          </p>
        ) : null}
      </section>

      {detail.state === "tombstoned" ? (
        <section
          className="rounded-xl border border-border/70 bg-muted/20 p-4 text-sm text-muted-foreground"
          data-testid="project-context-coordinate-tombstoned"
        >
          <Archive className="mb-2 h-4 w-4" />
          This Coordinate is tombstoned. Its stable identity and current Edge
          membership remain visible, but no active edit destination is offered.
        </section>
      ) : detail.state === "unavailable" ? (
        <section
          className="rounded-xl border border-warning/30 bg-warning/10 p-4 text-sm text-muted-foreground"
          data-testid="project-context-coordinate-unavailable"
        >
          <CloudOff className="mb-2 h-4 w-4" />
          {detail.unavailableReason ??
            "Current content is unavailable. This is not a tombstone or a Context Gap."}
        </section>
      ) : null}

      {detail.state === "active" && object && matchingView ? (
        <ProjectViewContent
          object={object}
          onOpenProjectView={onOpenProjectView}
          projectViewResult={matchingView}
        />
      ) : detail.state === "active" &&
        coordinate.type === "document" &&
        documentDetail ? (
        <ProjectContextDocumentContent
          detail={documentDetail}
          identity={projectContextDocumentIdentity(result)}
          onAnnouncement={onAnnouncement}
          onOpenDocument={onOpenDocument}
        />
      ) : (detail.state === "terminal" || detail.state === "active") &&
        coordinate.type === "meeting" ? (
        <ProjectContextMeetingContent
          fallback={detail.meeting}
          meetingId={coordinate.meetingId}
          onOpenMeeting={onOpenMeeting}
          title={detail.title}
        />
      ) : detail.state === "active" ? (
        <section
          className="rounded-xl border border-warning/30 bg-warning/10 p-4 text-sm text-muted-foreground"
          data-testid="project-context-coordinate-content-unavailable"
        >
          <CloudOff className="mb-2 h-4 w-4" />
          The Coordinate identity is verified, but a matching current Project
          View snapshot is not available. The Context graph remains intact.
        </section>
      ) : null}

      {detail.state === "tombstoned" || detail.state === "unavailable" ? (
        <section className="grid grid-cols-2 gap-3 rounded-xl border border-border/70 bg-muted/20 p-3">
          <Detail label="Known revision">
            {detail.objectRevision ?? detail.documentRevision ?? "Unknown"}
          </Detail>
          <Detail label="Last observed">
            {formatDateTime(detail.updatedAt)}
          </Detail>
          {detail.updatedBy ? (
            <div className="col-span-2">
              <Detail label="Actor">
                <ProjectViewActor compact pubkey={detail.updatedBy} />
              </Detail>
            </div>
          ) : null}
        </section>
      ) : null}

      <section>
        <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Coordinate ID
        </div>
        <code className="mt-1 block break-all text-xs text-muted-foreground">
          {stableId}
        </code>
      </section>

      <section className="space-y-2">
        <h3 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Current result membership · {edgeKeys.length}
        </h3>
        {edgeKeys.length > 0 ? (
          edgeKeys.map((edgeKey) => (
            <button
              className="flex w-full items-center gap-2 rounded-lg border border-border/70 bg-muted/20 px-3 py-2 text-left hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              data-testid={`project-context-coordinate-edge-${edgeKey}`}
              key={edgeKey}
              onClick={() => onSelectEdge(edgeKey)}
              type="button"
            >
              <span className="min-w-0 flex-1 truncate font-mono text-xs">
                {edgeKey}
              </span>
              <ArrowRight className="h-3.5 w-3.5 shrink-0" />
            </button>
          ))
        ) : (
          <p className="text-sm text-muted-foreground">
            This Query Anchor has no matching Edge in the current result.
          </p>
        )}
      </section>
    </div>
  );
}
