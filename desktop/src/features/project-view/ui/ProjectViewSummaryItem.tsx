import { ArrowRight } from "lucide-react";

import type {
  ProjectViewDocumentSummaryItem as DocumentSummaryItem,
  ProjectViewObjectSummaryItem as ObjectSummaryItem,
} from "@/features/project-view/explorerModel";
import { Badge } from "@/shared/ui/badge";
import { Markdown } from "@/shared/ui/markdown";

export type ProjectViewSummaryEntry = ObjectSummaryItem | DocumentSummaryItem;

/** A one-layer navigation card whose visible domain fields stay type/title/summary only. */
export function ProjectViewSummaryItem({
  item,
  onSelect,
}: {
  item: ProjectViewSummaryEntry;
  onSelect: (item: ProjectViewSummaryEntry) => void;
}) {
  return (
    <article
      className="group relative min-w-0 overflow-hidden rounded-xl border border-border/70 bg-card/60 p-4 transition-colors hover:bg-muted/30"
      data-kind={item.kind}
      data-testid="project-view-summary-item"
    >
      <button
        aria-label={`Open ${item.typeLabel}: ${item.title}`}
        className="absolute inset-0 z-10 rounded-xl focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        data-occurrence-key={item.occurrenceKey}
        onClick={() => onSelect(item)}
        type="button"
      >
        <span className="sr-only">
          Open {item.typeLabel}: {item.title}
        </span>
      </button>
      <div className="pointer-events-none flex min-w-0 items-start gap-3">
        <div className="min-w-0 flex-1">
          <Badge variant="outline">{item.typeLabel}</Badge>
          <h3 className="mt-2 break-words text-sm font-semibold leading-tight">
            {item.title}
          </h3>
          {item.summary ? (
            <Markdown
              className="mt-2 text-xs leading-relaxed text-muted-foreground"
              content={item.summary}
              interactive={false}
            />
          ) : (
            <p className="mt-2 text-xs italic text-muted-foreground">
              No summary provided.
            </p>
          )}
        </div>
        <ArrowRight
          aria-hidden="true"
          className="mt-1 h-4 w-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5"
        />
      </div>
    </article>
  );
}
