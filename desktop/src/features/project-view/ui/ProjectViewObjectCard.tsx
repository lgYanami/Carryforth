import { AlertCircle, ArrowUpRight, Link2 } from "lucide-react";

import {
  formatProjectViewTerm,
  projectViewObjectDescription,
  projectViewObjectPriority,
  projectViewObjectStatus,
  projectViewObjectTitle,
  projectViewObjectTypeLabel,
} from "@/features/project-view/model";
import type { ProjectViewObject } from "@/shared/api/tauriProjectView";
import { cn } from "@/shared/lib/cn";
import { Badge } from "@/shared/ui/badge";

type ProjectViewObjectCardProps = {
  issueReferenceCount?: number;
  object: ProjectViewObject;
  onSelect: (objectId: string) => void;
  selected?: boolean;
  size?: "default" | "compact";
};

function statusVariant(status: string | undefined) {
  if (status === "active" || status === "in_progress" || status === "ready") {
    return "info" as const;
  }
  if (
    status === "completed" ||
    status === "satisfied" ||
    status === "resolved"
  ) {
    return "success" as const;
  }
  if (status === "paused" || status === "submitted" || status === "planned") {
    return "warning" as const;
  }
  return "secondary" as const;
}

export function ProjectViewObjectCard({
  issueReferenceCount = 0,
  object,
  onSelect,
  selected = false,
  size = "default",
}: ProjectViewObjectCardProps) {
  const status = projectViewObjectStatus(object);
  const priority = projectViewObjectPriority(object);
  const title = projectViewObjectTitle(object);
  const description = projectViewObjectDescription(object);

  return (
    <button
      aria-label={`Inspect ${projectViewObjectTypeLabel(object.objectType)} ${title}`}
      className={cn(
        "group relative w-full rounded-xl border bg-card/70 text-left shadow-xs transition-colors hover:border-primary/40 hover:bg-card focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring",
        selected
          ? "border-primary/60 ring-1 ring-primary/30"
          : "border-border/70",
        size === "compact" ? "p-2.5" : "p-3",
      )}
      data-object-id={object.id}
      onClick={() => onSelect(object.id)}
      type="button"
    >
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              {projectViewObjectTypeLabel(object.objectType)}
            </span>
            {status ? (
              <Badge variant={statusVariant(status)}>
                {formatProjectViewTerm(status)}
              </Badge>
            ) : null}
            {priority && priority !== "normal" ? (
              <Badge
                variant={
                  priority === "urgent" || priority === "high"
                    ? "warning"
                    : "outline"
                }
              >
                {formatProjectViewTerm(priority)}
              </Badge>
            ) : null}
          </div>
          <div className="mt-1 flex min-w-0 items-start gap-1.5">
            <h3 className="min-w-0 flex-1 text-sm font-semibold leading-snug text-foreground">
              {title}
            </h3>
            <ArrowUpRight className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100" />
          </div>
          {description ? (
            <p
              className={cn(
                "mt-1 text-xs leading-relaxed text-muted-foreground",
                size === "compact" && "line-clamp-2",
              )}
            >
              {description}
            </p>
          ) : null}
        </div>
      </div>
      {issueReferenceCount > 0 ? (
        <div className="mt-2 flex items-center gap-1 text-xs text-amber-600 dark:text-amber-400">
          <AlertCircle className="h-3.5 w-3.5" />
          <span>
            {issueReferenceCount} related{" "}
            {issueReferenceCount === 1 ? "issue" : "issues"}
          </span>
          <Link2 className="h-3 w-3" />
        </div>
      ) : null}
    </button>
  );
}
