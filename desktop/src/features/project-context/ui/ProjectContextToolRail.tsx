import {
  AlertTriangle,
  LoaderCircle,
  Network,
  PanelRight,
  Sparkles,
} from "lucide-react";
import type * as React from "react";

import type { ProjectContextWorkspaceTool } from "@/features/project-context/workspacePanelModel";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";

export type ProjectContextSemanticToolStatus =
  | "idle"
  | "running"
  | "active"
  | "stale";

export type ProjectContextToolButtonRefs = Partial<
  Record<ProjectContextWorkspaceTool, React.RefObject<HTMLButtonElement | null>>
>;

export const PROJECT_CONTEXT_TOOL_TEST_IDS = {
  rail: "project-context-tools-rail",
  structure: "project-context-tool-structure",
  semantic: "project-context-tool-semantic",
  details: "project-context-tool-details",
} as const;

const TOOL_DEFINITIONS = [
  {
    tool: "structure",
    label: "Structure",
    description: "Query and inspect the current graph structure.",
    icon: Network,
  },
  {
    tool: "semantic",
    label: "Semantic",
    description: "Find paths related to a natural-language problem.",
    icon: Sparkles,
  },
  {
    tool: "details",
    label: "Details",
    description: "Inspect the selected Coordinate or Context Edge.",
    icon: PanelRight,
  },
] as const satisfies ReadonlyArray<{
  tool: ProjectContextWorkspaceTool;
  label: string;
  description: string;
  icon: React.ComponentType<{ className?: string }>;
}>;

function semanticStatusLabel(status: ProjectContextSemanticToolStatus) {
  switch (status) {
    case "running":
      return "Semantic query running";
    case "active":
      return "Semantic result active";
    case "stale":
      return "Semantic result stale";
    case "idle":
      return null;
  }
}

function SemanticStatusBadge({
  status,
}: {
  status: ProjectContextSemanticToolStatus;
}) {
  const label = semanticStatusLabel(status);
  if (!label) return null;

  return (
    <span
      aria-label={label}
      className={cn(
        "absolute right-0.5 top-0.5 flex h-3 w-3 items-center justify-center rounded-full border border-background",
        status === "stale"
          ? "bg-warning text-warning-foreground"
          : "bg-primary text-primary-foreground",
      )}
      data-semantic-status={status}
      role="img"
    >
      {status === "running" ? (
        <LoaderCircle className="h-2.5 w-2.5 animate-spin motion-reduce:animate-none" />
      ) : status === "stale" ? (
        <AlertTriangle className="h-2.5 w-2.5" />
      ) : (
        <span
          aria-hidden="true"
          className="h-1.5 w-1.5 rounded-full bg-current"
        />
      )}
    </span>
  );
}

/** One mutually-exclusive disclosure Rail shared by docked and modal shells. */
export function ProjectContextToolRail({
  activeTool,
  buttonRefs,
  className,
  detailsUnavailableReason = "Select a Coordinate or Context Edge to inspect details.",
  expanded,
  onToolToggle,
  panelId,
  selectionAvailable,
  semanticStatus = "idle",
}: {
  activeTool: ProjectContextWorkspaceTool;
  buttonRefs?: ProjectContextToolButtonRefs;
  className?: string;
  detailsUnavailableReason?: string;
  expanded: boolean;
  onToolToggle: (tool: ProjectContextWorkspaceTool) => void;
  panelId: string;
  selectionAvailable: boolean;
  semanticStatus?: ProjectContextSemanticToolStatus;
}) {
  return (
    <nav
      aria-label="Project Context tools"
      className={cn(
        "flex w-12 shrink-0 flex-col items-center gap-1 border border-border/70 bg-background/90 p-1.5 shadow-lg backdrop-blur-md",
        className,
      )}
      data-testid={PROJECT_CONTEXT_TOOL_TEST_IDS.rail}
    >
      {TOOL_DEFINITIONS.map((definition) => {
        const isOpen = expanded && activeTool === definition.tool;
        const detailsUnavailable =
          definition.tool === "details" && !selectionAvailable;
        const statusLabel =
          definition.tool === "semantic"
            ? semanticStatusLabel(semanticStatus)
            : null;
        const tooltip = detailsUnavailable
          ? detailsUnavailableReason
          : statusLabel
            ? `${definition.description} ${statusLabel}.`
            : definition.description;
        const Icon = definition.icon;

        return (
          <Tooltip key={definition.tool}>
            <TooltipTrigger asChild>
              <Button
                aria-controls={panelId}
                aria-disabled={detailsUnavailable || undefined}
                aria-expanded={isOpen}
                aria-label={
                  statusLabel
                    ? `${definition.label}, ${statusLabel}`
                    : definition.label
                }
                aria-pressed={isOpen}
                className={cn(
                  "relative shrink-0",
                  isOpen &&
                    "after:absolute after:bottom-1 after:left-0 after:top-1 after:w-0.5 after:rounded-full after:bg-current",
                )}
                data-testid={PROJECT_CONTEXT_TOOL_TEST_IDS[definition.tool]}
                onClick={() => {
                  if (!detailsUnavailable) onToolToggle(definition.tool);
                }}
                ref={buttonRefs?.[definition.tool]}
                size="icon"
                type="button"
                variant={isOpen ? "secondary" : "ghost"}
              >
                <Icon />
                {definition.tool === "semantic" ? (
                  <SemanticStatusBadge status={semanticStatus} />
                ) : null}
                <span className="sr-only">
                  {isOpen ? "Collapse" : "Open"} {definition.label}
                </span>
              </Button>
            </TooltipTrigger>
            <TooltipContent side="left">
              <p className="font-medium">{definition.label}</p>
              <p>{tooltip}</p>
            </TooltipContent>
          </Tooltip>
        );
      })}
    </nav>
  );
}
