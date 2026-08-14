import {
  Archive,
  ArrowUpRight,
  FileWarning,
  LoaderCircle,
  ShieldCheck,
} from "lucide-react";
import type * as React from "react";

import { useProjectDocument } from "@/features/project-documents/hooks";
import type {
  ProjectViewDocumentPage,
  ProjectViewExplorerSelection,
} from "@/features/project-view/explorerModel";
import { ProjectViewActor } from "@/features/project-view/ui/ProjectViewActor";
import { ProjectViewParentNavigation } from "@/features/project-view/ui/ProjectViewParentNavigation";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { ProjectDocumentIdentity } from "@/shared/api/tauriProjectDocument";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Markdown } from "@/shared/ui/markdown";

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function formatDateTime(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : dateTimeFormatter.format(date);
}

function shortDocumentId(documentId: string) {
  return documentId.length > 12 ? documentId.slice(0, 8) : documentId;
}

function coordinateTitle(page: ProjectViewDocumentPage) {
  const base = `Document ${shortDocumentId(page.coordinate.documentId)}`;
  return page.coordinate.mode === "pinned"
    ? `${base} · pinned revision ${page.coordinate.revision ?? "?"}`
    : base;
}

function documentTypeLabel(page: ProjectViewDocumentPage) {
  if (page.coordinate.relation === "resource_guide") return "Guide Document";
  return page.coordinate.mode === "pinned" ? "Pinned Document" : "Document";
}

/** Fetch and render an exact Document coordinate only while it is the Current Item. */
export function ProjectViewCurrentDocument({
  actorProfiles,
  currentPubkey,
  headingRef,
  identity,
  identityLoading = false,
  onNavigate,
  onOpenInDocuments,
  page,
}: {
  actorProfiles?: UserProfileLookup;
  currentPubkey?: string;
  headingRef?: React.Ref<HTMLHeadingElement>;
  identity?: ProjectDocumentIdentity;
  identityLoading?: boolean;
  onNavigate: (selection: ProjectViewExplorerSelection) => void;
  onOpenInDocuments: (search: { document: string; revision?: number }) => void;
  page: ProjectViewDocumentPage;
}) {
  const documentQuery = useProjectDocument({
    documentId: page.coordinate.documentId,
    enabled: Boolean(identity),
    identity,
    revision: page.coordinate.revision,
  });
  const document = documentQuery.data;
  const activeDocument = document?.state === "active" ? document : undefined;
  const title = activeDocument?.title?.trim() || coordinateTitle(page);
  const pinned = page.coordinate.mode === "pinned";

  return (
    <article
      className="mx-auto w-full max-w-6xl space-y-6 px-5 py-6"
      data-document-id={page.coordinate.documentId}
      data-testid="project-view-current-document"
    >
      <header className="space-y-4 border-b border-border/70 pb-6">
        <div className="flex min-w-0 items-start gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <Badge variant="outline">{documentTypeLabel(page)}</Badge>
              <Badge variant={pinned ? "info" : "success"}>
                {pinned ? "Pinned" : "Live"}
              </Badge>
              {document ? (
                <Badge variant="outline">
                  Revision {document.documentRevision}
                </Badge>
              ) : null}
            </div>
            <h1
              className="mt-3 break-words text-2xl font-semibold tracking-tight outline-hidden"
              data-occurrence-key={page.occurrenceKey}
              ref={headingRef}
              tabIndex={-1}
            >
              {title}
            </h1>
          </div>
          <ProjectViewParentNavigation
            onSelect={(parent) =>
              onNavigate({ kind: "object", objectId: parent.objectId })
            }
            parent={page.parent}
          />
        </div>

        {activeDocument?.summary ? (
          <section className="rounded-xl border border-border/70 bg-muted/20 p-4">
            <h2 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Summary
            </h2>
            <Markdown
              className="mt-2 text-sm leading-6"
              content={activeDocument.summary}
              interactive={false}
            />
          </section>
        ) : null}

        <div className="flex flex-wrap items-center gap-2">
          <Button
            data-testid="project-view-open-document"
            onClick={() => onOpenInDocuments(page.openInDocumentsSearch)}
            size="sm"
            type="button"
            variant="outline"
          >
            Open in Documents
            <ArrowUpRight />
          </Button>
          <span className="text-xs text-muted-foreground">
            {page.coordinate.relation === "resource_guide"
              ? "Resource guide reference"
              : pinned
                ? `Context reference · revision ${page.coordinate.revision ?? "unknown"}`
                : "Live Context reference"}
          </span>
        </div>
      </header>

      {identityLoading ? (
        <div
          aria-live="polite"
          className="flex items-center gap-2 rounded-xl border border-border/70 bg-muted/20 p-4 text-sm text-muted-foreground"
          data-testid="project-view-document-source-loading"
        >
          <LoaderCircle className="h-4 w-4 animate-spin" />
          Verifying the Document source…
        </div>
      ) : !identity ? (
        <div
          className="rounded-xl border border-warning/30 bg-warning/10 p-4 text-sm text-muted-foreground"
          data-testid="project-view-document-source-unavailable"
        >
          <FileWarning className="mb-2 h-4 w-4" />
          The verified Document source is unavailable, so no body request was
          issued.
        </div>
      ) : documentQuery.isPending ? (
        <div
          aria-live="polite"
          className="flex items-center gap-2 rounded-xl border border-border/70 bg-muted/20 p-4 text-sm text-muted-foreground"
          data-testid="project-view-document-loading"
        >
          <LoaderCircle className="h-4 w-4 animate-spin" />
          Verifying Document content…
        </div>
      ) : documentQuery.isError ? (
        <div
          className="rounded-xl border border-destructive/30 bg-destructive/10 p-4 text-sm text-muted-foreground"
          data-testid="project-view-document-error"
        >
          <FileWarning className="mb-2 h-4 w-4 text-destructive" />
          This exact Document coordinate could not be verified.
          {documentQuery.error instanceof Error ? (
            <p className="mt-2 text-xs">{documentQuery.error.message}</p>
          ) : null}
        </div>
      ) : document?.state === "deleted" ? (
        <div
          className="rounded-xl border border-border/70 bg-muted/20 p-5 text-sm text-muted-foreground"
          data-testid="project-view-document-deleted"
        >
          <Archive className="mb-2 h-4 w-4" />
          {pinned
            ? "This exact immutable revision is a tombstone and carries no Markdown body."
            : "The current verified Document is deleted and has no Markdown body."}
        </div>
      ) : activeDocument ? (
        <>
          <section className="grid gap-3 rounded-xl border border-border/70 bg-muted/20 p-4 sm:grid-cols-2">
            <div>
              <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                Verified revision
              </div>
              <div className="mt-1 text-sm">
                {activeDocument.documentRevision}
              </div>
            </div>
            <div>
              <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                Revised
              </div>
              <div className="mt-1 text-sm">
                {formatDateTime(activeDocument.revisionAt)}
              </div>
            </div>
            <div className="sm:col-span-2">
              <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                Revised by
              </div>
              <div className="mt-1 text-sm">
                <ProjectViewActor
                  currentPubkey={currentPubkey}
                  profiles={actorProfiles}
                  pubkey={activeDocument.revisionBy}
                />
              </div>
            </div>
            <div className="sm:col-span-2">
              <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                Document ID
              </div>
              <code className="mt-1 block break-all text-xs text-muted-foreground">
                {activeDocument.documentId}
              </code>
            </div>
          </section>

          <section
            className="overflow-hidden rounded-xl border border-border/70 bg-background"
            data-testid="project-view-document-body"
          >
            <div className="flex items-center gap-2 border-b border-border/70 bg-muted/20 px-4 py-3">
              <ShieldCheck className="h-4 w-4 text-emerald-600 dark:text-emerald-400" />
              <h2 className="text-sm font-semibold">Verified Markdown</h2>
            </div>
            <div className="p-5">
              <Markdown
                className="text-base leading-6"
                content={activeDocument.contentMarkdown ?? ""}
                interactive={false}
              />
            </div>
          </section>
        </>
      ) : null}
    </article>
  );
}
