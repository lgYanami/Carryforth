import type { LucideIcon } from "lucide-react";
import {
  Archive,
  BookOpen,
  CircleDot,
  ClipboardCheck,
  CloudOff,
  FileText,
  Flag,
  Hammer,
  Layers3,
  PackageOpen,
  Route,
  UsersRound,
} from "lucide-react";
import type { CSSProperties } from "react";
import type { NodeProps } from "@xyflow/react";

import type { ProjectContextCoordinateFlowNode } from "@/features/project-context/presentation";
import { ProjectContextNodeHandles } from "@/features/project-context/ui/ProjectContextNodeHandles";
import { cn } from "@/shared/lib/cn";

const OBJECT_ICONS: Record<string, LucideIcon> = {
  project_profile: BookOpen,
  goal: Flag,
  role: UsersRound,
  plan: Route,
  stage: Layers3,
  requirement: ClipboardCheck,
  issue: CircleDot,
  work: Hammer,
  resource: PackageOpen,
};

function coordinateIcon(data: ProjectContextCoordinateFlowNode["data"]) {
  if (data.coordinate.coordinate?.type === "document") return FileText;
  return data.coordinate.objectType
    ? (OBJECT_ICONS[data.coordinate.objectType] ?? CircleDot)
    : CircleDot;
}

/** Read-only visual node for one real Project Context Coordinate. */
export function ProjectContextCoordinateNode({
  data,
}: NodeProps<ProjectContextCoordinateFlowNode>) {
  const Icon = coordinateIcon(data);
  const isTombstoned = data.coordinate.state === "tombstoned";
  const isUnavailable = data.coordinate.state === "unavailable";
  const style = {
    "--project-context-island-hue": data.hue,
  } as CSSProperties;

  return (
    <div
      className={cn(
        "project-context-coordinate h-full w-full",
        isTombstoned && "project-context-coordinate--tombstoned",
        isUnavailable && "project-context-coordinate--unavailable",
      )}
      data-coordinate-key={data.coordinate.coordinateKey}
      data-emphasis={data.emphasis}
      data-island={data.islandIndex}
      data-lifecycle={data.coordinate.state}
      data-query-anchor={data.queryAnchor}
      data-testid={`project-context-coordinate-${data.coordinate.coordinateKey}`}
      style={style}
    >
      <ProjectContextNodeHandles type="target" />
      {data.queryAnchor ? (
        <span className="project-context-coordinate__anchor absolute -top-2.5 right-2 z-10 rounded-full px-2 py-0.5 text-3xs font-bold uppercase tracking-wider">
          Query anchor
        </span>
      ) : null}
      <button
        aria-label={`${data.queryAnchor ? "Query anchor, " : ""}Select ${data.coordinate.typeLabel} ${data.coordinate.displayTitle}`}
        className="nodrag nopan flex h-full w-full items-start gap-3 rounded-xl px-3 py-3 text-left outline-none"
        type="button"
      >
        <span className="project-context-coordinate__icon flex h-9 w-9 shrink-0 items-center justify-center rounded-lg">
          <Icon className="h-4 w-4" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            <span className="truncate text-sm font-semibold">
              {data.coordinate.displayTitle}
            </span>
          </span>
          <span className="mt-1 flex min-w-0 items-center gap-1.5 text-2xs text-muted-foreground">
            <span className="shrink-0 rounded-full border border-current/20 px-1.5 py-0.5 font-medium">
              {data.coordinate.typeLabel}
            </span>
            <span className="truncate font-mono">
              {data.coordinate.stableId}
            </span>
          </span>
          {isTombstoned || isUnavailable ? (
            <span className="mt-1.5 flex items-center gap-1 text-2xs font-medium">
              {isTombstoned ? (
                <Archive className="h-3 w-3" />
              ) : (
                <CloudOff className="h-3 w-3" />
              )}
              {isTombstoned ? "Tombstoned" : "Unavailable"}
            </span>
          ) : null}
        </span>
      </button>
    </div>
  );
}
