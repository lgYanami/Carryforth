import { Handle, Position } from "@xyflow/react";

import type { ProjectContextPort } from "@/features/project-context/layout";

const POSITIONS: Array<{ id: ProjectContextPort; position: Position }> = [
  { id: "top", position: Position.Top },
  { id: "right", position: Position.Right },
  { id: "bottom", position: Position.Bottom },
  { id: "left", position: Position.Left },
];

/** Invisible incidence ports keep Spokes attached to the nearest node edge. */
export function ProjectContextNodeHandles({
  type,
}: {
  type: "source" | "target";
}) {
  return POSITIONS.map(({ id, position }) => (
    <Handle
      className="project-context-handle"
      id={id}
      isConnectable={false}
      key={id}
      position={position}
      type={type}
    />
  ));
}
