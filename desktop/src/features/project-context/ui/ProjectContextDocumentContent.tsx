import {
  Archive,
  ArrowUpRight,
  CloudOff,
  FileWarning,
  LoaderCircle,
  ShieldCheck,
} from "lucide-react";
import * as React from "react";

import { useProjectDocument } from "@/features/project-documents/hooks";
import type { ProjectContextWorkspaceAnnouncementEvent } from "@/features/project-context/workspacePanelModel";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import type { ProjectContextDocumentDetail } from "@/shared/api/tauriProjectContext";
import type { ProjectDocumentIdentity } from "@/shared/api/tauriProjectDocument";
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

function Metadata({ detail }: { detail: ProjectContextDocumentDetail }) {
  return (
    <section className="grid grid-cols-2 gap-3 rounded-xl border border-border/70 bg-muted/20 p-3">
      <div>
        <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Context-observed revision
        </div>
        <div className="mt-1 text-sm">
          {detail.documentRevision ?? "Unknown"}
        </div>
      </div>
      <div>
        <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Observed at
        </div>
        <div className="mt-1 text-sm">{formatDateTime(detail.updatedAt)}</div>
      </div>
      {detail.updatedBy ? (
        <div className="col-span-2">
          <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Observed actor
          </div>
          <div className="mt-1 min-w-0 text-sm">
            <ProjectViewActor compact pubkey={detail.updatedBy} />
          </div>
        </div>
      ) : null}
    </section>
  );
}

/** One Context Document's metadata and current Markdown, read only after selection. */
export function ProjectContextDocumentContent({
  detail,
  identity,
  onAnnouncement,
  onOpenDocument,
}: {
  detail: ProjectContextDocumentDetail;
  identity?: ProjectDocumentIdentity;
  onAnnouncement?: (event: ProjectContextWorkspaceAnnouncementEvent) => void;
  onOpenDocument: (documentId: string) => void;
}) {
  const readable = detail.state === "active" && Boolean(identity);
  const documentQuery = useProjectDocument({
    documentId: detail.documentId,
    enabled: readable,
    identity,
  });
  const document = documentQuery.data;
  const currentDocument = document?.state === "active" ? document : undefined;
  const title =
    currentDocument?.title?.trim() ||
    detail.title?.trim() ||
    "Context Document";
  const summary = currentDocument?.summary;
  const canOpenDocument =
    detail.state === "active" && document?.state !== "deleted";

  React.useEffect(() => {
    if (!documentQuery.isError) return;
    onAnnouncement?.({
      key: `document:${detail.documentId}:verification-failed:${documentQuery.errorUpdatedAt}`,
      message: "Current Document content could not be verified.",
    });
  }, [
    detail.documentId,
    documentQuery.errorUpdatedAt,
    documentQuery.isError,
    onAnnouncement,
  ]);

  return (
    <div
      className="space-y-4"
      data-document-id={detail.documentId}
      data-testid="project-context-document-content"
    >
      <section>
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline">Context Document</Badge>
          {detail.state === "active" ? (
            <Badge variant="success">Active</Badge>
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
        <h3 className="mt-2 text-base font-semibold leading-tight">{title}</h3>
        {summary ? (
          <section
            className="mt-3 rounded-lg border border-border/70 bg-muted/20 p-3"
            data-testid="project-context-document-summary"
          >
            <h4 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Summary
            </h4>
            <Markdown
              className="mt-2 text-sm leading-6"
              content={summary}
              interactive={false}
            />
          </section>
        ) : null}
        <div className="mt-3 flex flex-wrap gap-2">
          {canOpenDocument ? (
            <Button
              data-testid={`project-context-open-document-${detail.documentId}`}
              onClick={() => onOpenDocument(detail.documentId)}
              size="sm"
              type="button"
              variant="outline"
            >
              Open in Documents
              <ArrowUpRight />
            </Button>
          ) : null}
        </div>
      </section>

      <Metadata detail={detail} />

      <section>
        <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
          Document ID
        </div>
        <code className="mt-1 block break-all text-xs text-muted-foreground">
          {detail.documentId}
        </code>
      </section>

      {detail.state === "tombstoned" ? (
        <div
          className="rounded-xl border border-border/70 bg-muted/20 p-4 text-sm text-muted-foreground"
          data-testid="project-context-document-tombstoned"
        >
          <Archive className="mb-2 h-4 w-4" />
          This verified binding points to a tombstoned Document. No active body
          is requested.
        </div>
      ) : detail.state === "unavailable" ? (
        <div
          className="rounded-xl border border-warning/30 bg-warning/10 p-4 text-sm text-muted-foreground"
          data-testid="project-context-document-unavailable"
        >
          <CloudOff className="mb-2 h-4 w-4" />
          {detail.unavailableReason ??
            "The Document identity remains verified, but its current content is unavailable."}
        </div>
      ) : !identity ? (
        <div
          className="rounded-xl border border-warning/30 bg-warning/10 p-4 text-sm text-muted-foreground"
          data-testid="project-context-document-source-unavailable"
        >
          <CloudOff className="mb-2 h-4 w-4" />
          The Context result did not observe a readable Document source, so
          Desktop will not issue a body request from this identity.
        </div>
      ) : documentQuery.isPending ? (
        <div
          className="flex items-center gap-2 rounded-xl border border-border/70 bg-muted/20 p-4 text-sm text-muted-foreground"
          data-testid="project-context-document-loading"
        >
          <LoaderCircle className="h-4 w-4 animate-spin" />
          Verifying current Markdown…
        </div>
      ) : documentQuery.isError ? (
        <div
          className="rounded-xl border border-destructive/30 bg-destructive/10 p-4 text-sm text-muted-foreground"
          data-testid="project-context-document-error"
        >
          <FileWarning className="mb-2 h-4 w-4 text-destructive" />
          The current body could not be verified. The Context Edge and binding
          above remain visible.
          {documentQuery.error instanceof Error ? (
            <p className="mt-2 text-xs">{documentQuery.error.message}</p>
          ) : null}
        </div>
      ) : document?.state === "deleted" ? (
        <div
          className="rounded-xl border border-border/70 bg-muted/20 p-4 text-sm text-muted-foreground"
          data-testid="project-context-document-current-tombstone"
        >
          <Archive className="mb-2 h-4 w-4" />
          The current verified Document is tombstoned. Its body is empty; the
          Context structure remains independently pinned.
        </div>
      ) : document ? (
        <section
          className="overflow-hidden rounded-xl border border-border/70 bg-background"
          data-testid={`project-context-document-body-${detail.documentId}`}
        >
          <div className="flex flex-wrap items-center gap-2 border-b border-border/70 bg-muted/20 px-3 py-2">
            <ShieldCheck className="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400" />
            <span className="text-xs font-semibold">Current verified body</span>
            <Badge className="ml-auto" variant="outline">
              Revision {document.documentRevision}
            </Badge>
          </div>
          <div className="border-b border-border/70 px-3 py-2 text-xs text-muted-foreground">
            Updated {formatDateTime(document.revisionAt)} by{" "}
            <ProjectViewActor compact pubkey={document.revisionBy} />
          </div>
          <div className="px-4 py-4">
            <Markdown
              className="text-sm leading-6"
              content={document.contentMarkdown ?? ""}
              interactive={false}
            />
          </div>
        </section>
      ) : null}
    </div>
  );
}
