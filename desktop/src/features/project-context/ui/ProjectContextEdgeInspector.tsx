import {
  Archive,
  ArrowRight,
  ArrowUpRight,
  CloudOff,
  FileText,
  Network,
} from "lucide-react";

import { projectContextInspectedEdge } from "@/features/project-context/inspectorModel";
import { projectViewObjectTypeLabel } from "@/features/project-view/model";
import type {
  ProjectContextCoordinateDetail,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Markdown } from "@/shared/ui/markdown";

function coordinateType(detail: ProjectContextCoordinateDetail) {
  if (detail.coordinate.type === "document") return "Document";
  if (detail.coordinate.type === "meeting") return "Meeting";
  return projectViewObjectTypeLabel(detail.coordinate.objectType);
}

function CoordinateRow({
  detail,
  onSelect,
}: {
  detail: ProjectContextCoordinateDetail;
  onSelect: (coordinateKey: string) => void;
}) {
  return (
    <button
      className="flex w-full items-start gap-2 rounded-lg border border-border/70 bg-muted/20 px-3 py-2.5 text-left transition-colors hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      data-testid={`project-context-edge-coordinate-${detail.coordinateKey}`}
      onClick={() => onSelect(detail.coordinateKey)}
      type="button"
    >
      <Network className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1">
        <span className="flex items-start gap-1.5">
          <span className="min-w-0 flex-1 text-sm font-medium">
            <span className="block truncate">
              {detail.title?.trim() || coordinateType(detail)}
            </span>
            {detail.summary ? (
              <span className="mt-1 block line-clamp-2 text-xs font-normal text-muted-foreground">
                {detail.summary}
              </span>
            ) : null}
          </span>
          <span className="flex shrink-0 flex-wrap items-center gap-1.5">
            <Badge variant="outline">{coordinateType(detail)}</Badge>
            {detail.state === "tombstoned" ? (
              <Badge variant="secondary">
                <Archive className="mr-1 h-3 w-3" />
                Tombstoned
              </Badge>
            ) : detail.state === "unavailable" ? (
              <Badge variant="warning">
                <CloudOff className="mr-1 h-3 w-3" />
                Unavailable
              </Badge>
            ) : null}
          </span>
        </span>
        <span className="mt-1 block truncate font-mono text-2xs text-muted-foreground">
          {detail.coordinateKey}
        </span>
      </span>
      <ArrowRight className="mt-0.5 h-3.5 w-3.5 shrink-0" />
    </button>
  );
}

/** Complete unordered Coordinate set and independently selected Context Documents. */
export function ProjectContextEdgeInspector({
  edgeKey,
  onOpenDocument,
  onSelectCoordinate,
  result,
}: {
  edgeKey: string;
  onOpenDocument: (documentId: string) => void;
  onSelectCoordinate: (coordinateKey: string) => void;
  result: ProjectContextQueryResult;
}) {
  const inspected = projectContextInspectedEdge(result, edgeKey);

  if (!inspected) {
    return (
      <div className="p-4 text-sm text-muted-foreground">
        This Context Edge is no longer present in the current verified result.
      </div>
    );
  }

  return (
    <div className="space-y-5 p-4" data-testid="project-context-edge-inspector">
      <section>
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline">Context Edge</Badge>
          <Badge variant="secondary">
            {inspected.coordinates.length} Coordinates
          </Badge>
          <Badge variant="outline">
            {inspected.documents.length} Context Documents
          </Badge>
        </div>
        <h3 className="mt-2 text-lg font-semibold">Context Edge</h3>
        <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
          One undirected relationship across the complete Coordinate set below.
        </p>
        <details className="mt-3 rounded-lg border border-border/70 bg-muted/20 px-3 py-2">
          <summary className="cursor-pointer text-xs font-semibold">
            Edge key diagnostic
          </summary>
          <code
            className="mt-2 block break-all text-xs text-muted-foreground"
            data-testid="project-context-edge-key"
          >
            {inspected.edge.edgeKey}
          </code>
        </details>
      </section>

      <section className="space-y-2">
        <h3 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Canonical unordered Coordinate set
        </h3>
        {inspected.coordinates.map((detail) => (
          <CoordinateRow
            detail={detail}
            key={detail.coordinateKey}
            onSelect={onSelectCoordinate}
          />
        ))}
      </section>

      <section className="space-y-2">
        <div className="flex items-center gap-2">
          <FileText className="h-4 w-4 text-muted-foreground" />
          <h3 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Context Documents · {inspected.documents.length}
          </h3>
        </div>
        {inspected.documents.length > 0 ? (
          <ul aria-label="Context Documents" className="space-y-1.5">
            {inspected.documents.map((document) => (
              <li key={document.documentId}>
                <details
                  className="group rounded-lg border border-border/70 bg-muted/20 open:bg-background"
                  data-testid={`project-context-edge-document-${document.documentId}`}
                >
                  <summary className="cursor-pointer list-none px-3 py-2.5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
                    <span className="flex min-w-0 items-start gap-2">
                      <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                      <span className="min-w-0 flex-1">
                        <span className="flex flex-wrap items-center gap-1.5">
                          <span className="min-w-0 flex-1 truncate text-sm font-medium">
                            {document.title?.trim() || "Context Document"}
                          </span>
                          {document.state === "tombstoned" ? (
                            <Badge variant="secondary">
                              <Archive className="mr-1 h-3 w-3" />
                              Tombstoned
                            </Badge>
                          ) : document.state === "unavailable" ? (
                            <Badge variant="warning">
                              <CloudOff className="mr-1 h-3 w-3" />
                              Unavailable
                            </Badge>
                          ) : null}
                        </span>
                        <span className="mt-0.5 block truncate font-mono text-2xs text-muted-foreground">
                          {document.documentId}
                        </span>
                      </span>
                    </span>
                  </summary>
                  <div
                    className="space-y-3 border-t border-border/70 px-3 py-3"
                    data-testid={`project-context-edge-document-panel-${document.documentId}`}
                  >
                    {document.summary ? (
                      <section
                        data-testid={`project-context-edge-document-summary-${document.documentId}`}
                      >
                        <h4 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                          Summary
                        </h4>
                        <Markdown
                          className="mt-2 text-sm leading-6"
                          content={document.summary}
                          interactive={false}
                        />
                      </section>
                    ) : null}
                    {document.state === "active" ? (
                      <Button
                        data-testid={`project-context-open-document-${document.documentId}`}
                        onClick={() => onOpenDocument(document.documentId)}
                        size="sm"
                        type="button"
                        variant="outline"
                      >
                        Open in Documents
                        <ArrowUpRight />
                      </Button>
                    ) : null}
                  </div>
                </details>
              </li>
            ))}
          </ul>
        ) : (
          <p
            className="rounded-lg border border-dashed border-border/70 p-3 text-sm text-muted-foreground"
            data-testid="project-context-edge-no-documents"
          >
            This Edge has no bound Context Documents.
          </p>
        )}
      </section>
    </div>
  );
}
