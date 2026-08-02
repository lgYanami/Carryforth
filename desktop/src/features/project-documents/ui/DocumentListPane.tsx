import { FileText, Plus } from "lucide-react";

import type { ProjectDocumentListItem } from "@/shared/api/tauriProjectDocument";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";

function formatDate(value: string): string {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime())
    ? value
    : new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(
        parsed,
      );
}

export function DocumentListPane({
  documents,
  isFetching,
  onCreate,
  onSelect,
  selectedDocumentId,
}: {
  documents: ProjectDocumentListItem[];
  isFetching: boolean;
  onCreate: () => void;
  onSelect: (documentId: string) => void;
  selectedDocumentId?: string;
}) {
  return (
    <aside className="flex w-72 shrink-0 flex-col border-r border-border/70 bg-muted/10">
      <div className="flex items-center gap-2 border-b border-border/70 px-3 py-3">
        <div className="min-w-0 flex-1">
          <h2 className="text-xs font-semibold">Active Documents</h2>
          <p className="text-2xs text-muted-foreground">
            {documents.length} metadata record
            {documents.length === 1 ? "" : "s"}
            {isFetching ? " · verifying…" : ""}
          </p>
        </div>
        <Button
          aria-label="Create Document"
          data-testid="document-create"
          onClick={onCreate}
          size="icon"
          type="button"
          variant="outline"
        >
          <Plus className="h-4 w-4" />
        </Button>
      </div>
      <div
        className="min-h-0 flex-1 overflow-auto p-2"
        data-testid="document-list"
      >
        {documents.length === 0 ? (
          <div className="px-3 py-8 text-center">
            <FileText className="mx-auto h-7 w-7 text-muted-foreground" />
            <p className="mt-2 text-xs font-medium">No active Documents</p>
            <p className="mt-1 text-2xs text-muted-foreground">
              Create a Markdown record when the project needs durable context.
            </p>
          </div>
        ) : (
          <div className="space-y-1">
            {documents.map((document) => (
              <button
                className={cn(
                  "w-full rounded-xl px-3 py-2.5 text-left transition-colors hover:bg-muted/70 focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring",
                  selectedDocumentId === document.documentId &&
                    "bg-primary/10 text-foreground",
                )}
                data-testid={`document-list-item-${document.documentId}`}
                key={document.documentId}
                onClick={() => onSelect(document.documentId)}
                type="button"
              >
                <span className="block truncate text-sm font-medium">
                  {document.title}
                </span>
                {document.summary ? (
                  <span className="mt-0.5 line-clamp-2 block text-xs text-muted-foreground">
                    {document.summary}
                  </span>
                ) : null}
                <span className="mt-1.5 flex items-center justify-between gap-2 text-2xs text-muted-foreground">
                  <span>Revision {document.documentRevision}</span>
                  <span>{formatDate(document.updatedAt)}</span>
                </span>
              </button>
            ))}
          </div>
        )}
      </div>
    </aside>
  );
}
