import { FileText } from "lucide-react";
import type { CSSProperties } from "react";
import type { NodeProps } from "@xyflow/react";

import type { ProjectContextHubFlowNode } from "@/features/project-context/presentation";
import { ProjectContextNodeHandles } from "@/features/project-context/ui/ProjectContextNodeHandles";

/** Visual incidence Hub for exactly one undirected domain Context Edge. */
export function ProjectContextEdgeHub({
  data,
}: NodeProps<ProjectContextHubFlowNode>) {
  const documentCount = data.hub.contextDocumentIds.length;
  const semanticDescription = data.semanticRoot
    ? "Semantic root, "
    : data.semanticEmphasis === "route"
      ? "In semantic path, "
      : data.semanticEmphasis === "member"
        ? "Semantic path member, "
        : "";
  const style = {
    "--project-context-island-hue": data.hue,
  } as CSSProperties;

  return (
    <div
      className="project-context-hub h-full w-full"
      data-context-graph-kind="edge"
      data-edge-key={data.hub.edgeKey}
      data-emphasis={data.emphasis}
      data-island={data.islandIndex}
      data-semantic-emphasis={data.semanticEmphasis}
      data-semantic-root={data.semanticRoot}
      data-testid={`project-context-edge-${data.hub.edgeKey}`}
      style={style}
    >
      <ProjectContextNodeHandles type="source" />
      {data.semanticRoot ? (
        <span
          aria-hidden
          className="project-context-semantic-marker project-context-semantic-marker--root absolute -top-2.5 left-1/2 z-20 -translate-x-1/2 whitespace-nowrap rounded-full px-2 py-0.5 text-3xs font-bold uppercase tracking-wider"
        >
          Semantic root
        </span>
      ) : null}
      <button
        aria-label={`${semanticDescription}Context Edge connecting ${data.hub.coordinateKeys.length} coordinates with ${documentCount} documents`}
        aria-pressed={data.selected}
        className="nodrag nopan relative flex h-full w-full items-center justify-center rounded-full outline-none"
        type="button"
      >
        <span aria-hidden className="project-context-hub__diamond" />
        <span className="relative z-10 flex flex-col items-center leading-none">
          <span className="text-2xs font-bold uppercase tracking-wider">
            Edge
          </span>
          <span className="mt-1 flex items-center gap-1 text-3xs font-medium">
            <FileText className="h-2.5 w-2.5" />
            {documentCount}
          </span>
        </span>
      </button>
    </div>
  );
}
