import {
  FileClock,
  History,
  Pencil,
  RotateCcw,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import * as React from "react";

import { useProjectDocumentMutation } from "@/features/project-documents/hooks";
import { DocumentHistory } from "@/features/project-documents/ui/DocumentHistory";
import type {
  ProjectDocument,
  ProjectDocumentIdentity,
} from "@/shared/api/tauriProjectDocument";
import { Alert, AlertDescription, AlertTitle } from "@/shared/ui/alert";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Markdown } from "@/shared/ui/markdown";
import { PubKey } from "@/shared/ui/PubKey";

function formatDate(value: string): string {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime())
    ? value
    : new Intl.DateTimeFormat(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(parsed);
}

export function DocumentViewer({
  currentRevision,
  document,
  identity,
  onDeleted,
  onEdit,
  onSelectRevision,
  selectedRevision,
}: {
  currentRevision: number;
  document: ProjectDocument;
  identity: ProjectDocumentIdentity;
  onDeleted: () => void;
  onEdit: () => void;
  onSelectRevision: (revision: number | undefined) => void;
  selectedRevision?: number;
}) {
  const [showHistory, setShowHistory] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState(false);
  const [deleteConflict, setDeleteConflict] = React.useState(false);
  const mutation = useProjectDocumentMutation();
  const pinned = selectedRevision !== undefined;

  async function deleteDocument() {
    setDeleteConflict(false);
    try {
      const result = await mutation.mutateAsync({
        identity,
        mutation: {
          type: "delete",
          documentId: document.documentId,
          expectedDocumentRevision: currentRevision,
        },
      });
      if (result.status === "applied") {
        onDeleted();
      } else {
        setDeleteConflict(true);
      }
    } catch {
      // React Query owns the visible error; keep confirmation open for retry.
    }
  }

  return (
    <div className="flex min-h-0 flex-1">
      <article
        className="min-w-0 flex-1 overflow-auto"
        data-testid="document-viewer"
      >
        <header className="border-b border-border/70 px-5 py-4">
          <div className="flex items-start gap-3">
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-2">
                <h1 className="text-lg font-semibold">
                  {document.title ?? "Deleted Document"}
                </h1>
                <Badge variant={pinned ? "info" : "success"}>
                  {pinned ? `Pinned r${document.documentRevision}` : "Current"}
                </Badge>
                <Badge variant="outline">
                  Revision {document.documentRevision}
                </Badge>
              </div>
              {document.summary ? (
                <p className="mt-1 text-sm text-muted-foreground">
                  {document.summary}
                </p>
              ) : null}
              <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-2xs text-muted-foreground">
                <span>{formatDate(document.revisionAt)}</span>
                <span className="inline-flex items-center gap-1">
                  by <PubKey pubkey={document.revisionBy} />
                </span>
                <span className="inline-flex items-center gap-1">
                  <ShieldCheck className="h-3 w-3" /> verified Relay projection
                </span>
              </div>
            </div>
            <div className="flex shrink-0 flex-wrap gap-2">
              {pinned ? (
                <Button
                  data-testid="document-return-current"
                  onClick={() => onSelectRevision(undefined)}
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  <RotateCcw className="h-4 w-4" />
                  Current
                </Button>
              ) : document.state === "active" ? (
                <Button
                  data-testid="document-edit"
                  onClick={onEdit}
                  size="sm"
                  type="button"
                  variant="outline"
                >
                  <Pencil className="h-4 w-4" />
                  Edit
                </Button>
              ) : null}
              <Button
                data-testid="document-toggle-history"
                onClick={() => setShowHistory((shown) => !shown)}
                size="sm"
                type="button"
                variant="outline"
              >
                <History className="h-4 w-4" />
                History
              </Button>
              {!pinned && document.state === "active" ? (
                <Button
                  data-testid="document-delete"
                  onClick={() => setConfirmDelete(true)}
                  size="sm"
                  type="button"
                  variant="ghost"
                >
                  <Trash2 className="h-4 w-4" />
                  Delete
                </Button>
              ) : null}
            </div>
          </div>
          {confirmDelete ? (
            <Alert className="mt-4 border-destructive/40 bg-destructive/10">
              <Trash2 className="h-4 w-4 text-destructive" />
              <AlertTitle>Append a permanent tombstone revision?</AlertTitle>
              <AlertDescription>
                The current Document will leave the active catalog. Its verified
                immutable history remains available by pinned revision.
                <div className="mt-3 flex gap-2">
                  <Button
                    data-testid="document-delete-confirm"
                    disabled={mutation.isPending}
                    onClick={() => void deleteDocument()}
                    size="sm"
                    type="button"
                    variant="destructive"
                  >
                    {mutation.isPending ? "Deleting…" : "Delete Document"}
                  </Button>
                  <Button
                    disabled={mutation.isPending}
                    onClick={() => setConfirmDelete(false)}
                    size="sm"
                    type="button"
                    variant="outline"
                  >
                    Cancel
                  </Button>
                </div>
                {mutation.error instanceof Error ? (
                  <p className="mt-2 text-destructive">
                    {mutation.error.message}
                  </p>
                ) : deleteConflict ? (
                  <p className="mt-2 text-destructive">
                    The Document changed before it could be deleted. Review the
                    refreshed current revision before trying again.
                  </p>
                ) : null}
              </AlertDescription>
            </Alert>
          ) : null}
        </header>
        <div className="px-5 py-5">
          {document.state === "deleted" ? (
            <div className="rounded-2xl border border-border/70 bg-muted/30 p-8 text-center">
              <FileClock className="mx-auto h-8 w-8 text-muted-foreground" />
              <h2 className="mt-3 text-sm font-semibold">Deleted revision</h2>
              <p className="mt-1 text-xs text-muted-foreground">
                This immutable tombstone carries no title, summary, or Markdown
                body.
              </p>
            </div>
          ) : (
            <div data-testid="document-markdown">
              <Markdown
                className="text-base leading-6"
                content={document.contentMarkdown ?? ""}
                interactive={false}
              />
            </div>
          )}
        </div>
      </article>
      {showHistory ? (
        <DocumentHistory
          currentRevision={currentRevision}
          documentId={document.documentId}
          identity={identity}
          onSelectRevision={onSelectRevision}
          selectedRevision={selectedRevision}
        />
      ) : null}
    </div>
  );
}
