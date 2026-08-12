import * as React from "react";
import type { EdgeMouseHandler, NodeMouseHandler } from "@xyflow/react";

import type { buildProjectContextGraph } from "@/features/project-context/graph";
import type {
  ProjectContextFlowEdge,
  ProjectContextFlowNode,
  ProjectContextGraphTarget,
} from "@/features/project-context/presentation";

function currentTextScale() {
  if (typeof document === "undefined") return 1;
  const fontSize = Number.parseFloat(
    window.getComputedStyle(document.documentElement).fontSize,
  );
  return Number.isFinite(fontSize) ? fontSize / 16 : 1;
}

/** Track the Desktop root text scale used to materialize graph geometry. */
export function useProjectContextTextScale() {
  const [scale, setScale] = React.useState(currentTextScale);
  React.useLayoutEffect(() => {
    const update = () => setScale(currentTextScale());
    const observer = new MutationObserver(update);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "style"],
    });
    window.addEventListener("resize", update);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", update);
    };
  }, []);
  return scale;
}

/** Map a selectable flow node back to its canonical route target. */
export function targetForProjectContextNode(
  node: ProjectContextFlowNode,
): ProjectContextGraphTarget | null {
  if (node.data.kind === "coordinate") {
    return { kind: "coordinate", key: node.data.coordinate.coordinateKey };
  }
  if (node.data.kind === "hub") {
    return { kind: "edge", key: node.data.hub.edgeKey };
  }
  return null;
}

/** Compare route targets without relying on object identity. */
export function sameProjectContextGraphTarget(
  left: ProjectContextGraphTarget | null,
  right: ProjectContextGraphTarget | null,
) {
  return left?.kind === right?.kind && left?.key === right?.key;
}

/** Human-readable label for the current route selection in the canvas HUD. */
export function projectContextSelectedLabel(
  graph: ReturnType<typeof buildProjectContextGraph>,
  selection: ProjectContextGraphTarget | null,
) {
  if (!selection) return undefined;
  if (selection.kind === "coordinate") {
    return graph.coordinates.find(
      (coordinate) => coordinate.coordinateKey === selection.key,
    )?.displayTitle;
  }
  const hub = graph.hubs.find(
    (candidate) => candidate.edgeKey === selection.key,
  );
  return hub
    ? `Edge · ${hub.coordinateKeys.length} ${hub.coordinateKeys.length === 1 ? "coordinate" : "coordinates"} · ${hub.contextDocumentIds.length} ${hub.contextDocumentIds.length === 1 ? "doc" : "docs"}`
    : undefined;
}

/** Unique bound Context Document count for compact graph summaries. */
export function projectContextDocumentCount(
  graph: ReturnType<typeof buildProjectContextGraph>,
) {
  return new Set(graph.hubs.flatMap((hub) => hub.contextDocumentIds)).size;
}

/** Remove transient hover emphasis without touching selection or semantics. */
export function clearProjectContextGraphHover(root: HTMLElement | null) {
  for (const element of root?.querySelectorAll("[data-context-graph-kind]") ??
    []) {
    element.removeAttribute("data-hover-emphasis");
  }
}

/** Apply incidence-neighbour hover emphasis to the rendered graph DOM. */
export function applyProjectContextGraphHover(
  root: HTMLElement | null,
  graph: ReturnType<typeof buildProjectContextGraph>,
  target: ProjectContextGraphTarget,
) {
  if (!root) return;
  const activeCoordinateKeys = new Set<string>();
  const activeEdgeKeys = new Set<string>();
  if (target.kind === "edge") {
    activeEdgeKeys.add(target.key);
    for (const key of graph.hubs.find((hub) => hub.edgeKey === target.key)
      ?.coordinateKeys ?? []) {
      activeCoordinateKeys.add(key);
    }
  } else {
    activeCoordinateKeys.add(target.key);
    for (const hub of graph.hubs) {
      if (hub.coordinateKeys.includes(target.key)) {
        activeEdgeKeys.add(hub.edgeKey);
      }
    }
  }

  for (const element of root.querySelectorAll("[data-context-graph-kind]")) {
    const kind = element.getAttribute("data-context-graph-kind");
    const coordinateKey = element.getAttribute("data-coordinate-key");
    const edgeKey = element.getAttribute("data-edge-key");
    const active =
      kind === "coordinate"
        ? coordinateKey !== null && activeCoordinateKeys.has(coordinateKey)
        : kind === "edge"
          ? edgeKey !== null && activeEdgeKeys.has(edgeKey)
          : target.kind === "edge"
            ? edgeKey === target.key
            : coordinateKey === target.key;
    element.setAttribute("data-hover-emphasis", active ? "active" : "dimmed");
  }
}

/** Stable React Flow pointer handlers for selection and transient incidence hover. */
export function useProjectContextPointerInteractions({
  graph,
  onSelectionChange,
  rootRef,
  selection,
}: {
  graph: ReturnType<typeof buildProjectContextGraph>;
  onSelectionChange: (selection: ProjectContextGraphTarget | null) => void;
  rootRef: React.RefObject<HTMLElement | null>;
  selection: ProjectContextGraphTarget | null;
}) {
  const onNodeClick = React.useCallback<
    NodeMouseHandler<ProjectContextFlowNode>
  >(
    (_event, node) => {
      const target = targetForProjectContextNode(node);
      if (!target) return;
      onSelectionChange(
        sameProjectContextGraphTarget(selection, target) ? null : target,
      );
    },
    [onSelectionChange, selection],
  );
  const onNodeMouseEnter = React.useCallback<
    NodeMouseHandler<ProjectContextFlowNode>
  >(
    (_event, node) => {
      const target = targetForProjectContextNode(node);
      if (target && !selection) {
        applyProjectContextGraphHover(rootRef.current, graph, target);
      }
    },
    [graph, rootRef, selection],
  );
  const onNodeMouseLeave = React.useCallback<
    NodeMouseHandler<ProjectContextFlowNode>
  >(() => clearProjectContextGraphHover(rootRef.current), [rootRef]);
  const onEdgeClick = React.useCallback<
    EdgeMouseHandler<ProjectContextFlowEdge>
  >(
    (_event, edge) => {
      if (!edge.data) return;
      const target = { kind: "edge", key: edge.data.edgeKey } as const;
      onSelectionChange(
        sameProjectContextGraphTarget(selection, target) ? null : target,
      );
    },
    [onSelectionChange, selection],
  );
  const onEdgeMouseEnter = React.useCallback<
    EdgeMouseHandler<ProjectContextFlowEdge>
  >(
    (_event, edge) => {
      if (edge.data && !selection) {
        applyProjectContextGraphHover(rootRef.current, graph, {
          kind: "edge",
          key: edge.data.edgeKey,
        });
      }
    },
    [graph, rootRef, selection],
  );
  const onEdgeMouseLeave = React.useCallback<
    EdgeMouseHandler<ProjectContextFlowEdge>
  >(() => clearProjectContextGraphHover(rootRef.current), [rootRef]);
  const onPaneClick = React.useCallback(
    () => onSelectionChange(null),
    [onSelectionChange],
  );

  return {
    onEdgeClick,
    onEdgeMouseEnter,
    onEdgeMouseLeave,
    onNodeClick,
    onNodeMouseEnter,
    onNodeMouseLeave,
    onPaneClick,
  };
}
