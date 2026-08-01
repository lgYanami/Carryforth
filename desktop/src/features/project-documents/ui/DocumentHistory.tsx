import { FileClock } from "lucide-react";

import { useProjectDocumentHistory } from "@/features/project-documents/hooks";
import type { ProjectDocumentIdentity } from "@/shared/api/tauriProjectDocument";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

function formatDate(value: string): string {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime())
    ? value
    : new Intl.DateTimeFormat(undefined, {
        dateStyle: "medium",
        timeStyle: "short",
      }).format(parsed);
}

export function DocumentHistory({
  currentRevision,
  documentId,
  identity,
  onSelectRevision,
  selectedRevision,
}: {
  currentRevision: number;
  documentId: string;
  identity: ProjectDocumentIdentity;
  onSelectRevision: (revision: number | undefined) => void;
  selectedRevision?: number;
}) {
  const history = useProjectDocumentHistory({
    identity,
    documentId,
    maxDocumentRevision: currentRevision,
  });
  return (
    <aside
      className="w-64 shrink-0 overflow-auto border-l border-border/70 bg-muted/10 p-3"
      data-testid="document-history"
    >
      <div className="mb-3 flex items-center gap-2 px-1">
        <FileClock className="h-4 w-4 text-muted-foreground" />
        <h2 className="text-xs font-semibold">Revision history</h2>
      </div>
      {history.isLoading ? (
        <p className="px-1 text-xs text-muted-foreground">Loading history…</p>
      ) : history.error instanceof Error ? (
        <p className="px-1 text-xs text-destructive">{history.error.message}</p>
      ) : (
        <div className="space-y-1">
          {history.data?.revisions.map((revision) => {
            const isCurrent = revision.documentRevision === currentRevision;
            const isSelected = selectedRevision
              ? revision.documentRevision === selectedRevision
              : isCurrent;
            return (
              <Button
                className="h-auto w-full justify-start px-2 py-2 text-left"
                data-testid={`document-history-r${revision.documentRevision}`}
                key={revision.revisionEventId}
                onClick={() =>
                  onSelectRevision(
                    isCurrent ? undefined : revision.documentRevision,
                  )
                }
                type="button"
                variant={isSelected ? "secondary" : "ghost"}
              >
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-1.5">
                    <span className="text-xs font-semibold">
                      Revision {revision.documentRevision}
                    </span>
                    {isCurrent ? (
                      <Badge variant="success">Current</Badge>
                    ) : null}
                    {revision.state === "deleted" ? (
                      <Badge variant="destructive">Deleted</Badge>
                    ) : null}
                  </span>
                  <span className="mt-1 block text-2xs font-normal text-muted-foreground">
                    {formatDate(revision.canonicalAt)}
                  </span>
                </span>
              </Button>
            );
          })}
        </div>
      )}
    </aside>
  );
}
