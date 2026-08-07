import type { CSSProperties } from "react";
import type { NodeProps } from "@xyflow/react";

import type { ProjectContextIslandFlowNode } from "@/features/project-context/presentation";

/** Presentation-only boundary and factual label for one connected component. */
export function ProjectContextIsland({
  data,
}: NodeProps<ProjectContextIslandFlowNode>) {
  const { island } = data;
  const style = {
    "--project-context-island-hue": data.hue,
  } as CSSProperties;
  return (
    <section
      aria-label={`Island ${island.index}, ${island.coordinateKeys.length} coordinates, ${island.edgeKeys.length} edges, ${island.contextDocumentIds.length} context documents`}
      className="project-context-island h-full w-full"
      data-island={island.index}
      data-testid={`project-context-island-${island.index}`}
      style={style}
    >
      <div className="project-context-island__label text-xs">
        <span className="project-context-island__ordinal text-2xs">
          {island.index}
        </span>
        <span>
          <span className="font-semibold">Island {island.index}</span>
          <span className="ml-2 text-muted-foreground">
            {island.coordinateKeys.length}{" "}
            {island.coordinateKeys.length === 1 ? "coordinate" : "coordinates"}{" "}
            · {island.edgeKeys.length}{" "}
            {island.edgeKeys.length === 1 ? "edge" : "edges"} ·{" "}
            {island.contextDocumentIds.length} context{" "}
            {island.contextDocumentIds.length === 1 ? "doc" : "docs"}
          </span>
        </span>
      </div>
    </section>
  );
}
