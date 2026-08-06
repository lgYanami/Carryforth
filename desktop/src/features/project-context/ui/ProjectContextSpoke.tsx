import { BaseEdge, getBezierPath, type EdgeProps } from "@xyflow/react";
import type { CSSProperties } from "react";

import type { ProjectContextFlowEdge } from "@/features/project-context/presentation";

/** One clickable, arrowless incidence segment belonging to a complete Edge. */
export function ProjectContextSpoke({
  data,
  id,
  interactionWidth,
  sourcePosition,
  sourceX,
  sourceY,
  targetPosition,
  targetX,
  targetY,
}: EdgeProps<ProjectContextFlowEdge>) {
  const [path] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
    curvature: 0.3,
  });
  const style = {
    "--project-context-island-hue": data?.hue ?? 267,
  } as CSSProperties;
  return (
    <BaseEdge
      className="project-context-spoke"
      data-edge-key={data?.edgeKey}
      data-emphasis={data?.emphasis ?? "normal"}
      data-testid={`project-context-spoke-${id}`}
      id={id}
      interactionWidth={interactionWidth ?? 28}
      path={path}
      style={style}
    />
  );
}
