import * as React from "react";

import { diffDocumentLines } from "@/features/project-documents/lineDiff";
import { cn } from "@/shared/lib/cn";

export function DocumentDiff({
  after,
  before,
  label,
}: {
  after: string;
  before: string;
  label: string;
}) {
  const rows = React.useMemo(
    () => diffDocumentLines(before, after),
    [after, before],
  );
  return (
    <section className="overflow-hidden rounded-xl border border-border/70">
      <div className="border-b border-border/70 bg-muted/40 px-3 py-2 text-xs font-semibold">
        {label}
      </div>
      <div
        className="max-h-72 overflow-auto bg-card/40 font-mono text-xs"
        data-testid="document-exact-diff"
      >
        {rows.length === 0 ? (
          <div className="px-3 py-4 text-muted-foreground">No changes.</div>
        ) : (
          rows.map((row) => (
            <div
              className={cn(
                "grid grid-cols-[3rem_3rem_1.5rem_minmax(0,1fr)] border-b border-border/30 px-2 py-0.5",
                row.kind === "delete" && "bg-destructive/10 text-destructive",
                row.kind === "insert" && "bg-success/10 text-success",
              )}
              key={`${row.kind}-${row.oldLine ?? ""}-${row.newLine ?? ""}`}
            >
              <span className="select-none text-right text-muted-foreground">
                {row.oldLine ?? ""}
              </span>
              <span className="select-none text-right text-muted-foreground">
                {row.newLine ?? ""}
              </span>
              <span className="select-none text-center">
                {row.kind === "delete"
                  ? "−"
                  : row.kind === "insert"
                    ? "+"
                    : " "}
              </span>
              <span className="whitespace-pre-wrap wrap-anywhere">
                {row.text}
              </span>
            </div>
          ))
        )}
      </div>
    </section>
  );
}
