import type {
  ProjectContextCoordinate,
  ProjectContextCoordinateDetail,
  ProjectContextDetailState,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import { projectContextCoordinateKey } from "@/shared/api/tauriProjectContext";

export type ProjectContextGraphCoordinate = {
  id: string;
  coordinateKey: string;
  coordinate?: ProjectContextCoordinate;
  displayTitle: string;
  stableId: string;
  state: ProjectContextDetailState;
  typeLabel: string;
  objectType?: string;
  unavailableReason?: string;
};

export type ProjectContextGraphHub = {
  id: string;
  edgeKey: string;
  coordinateKeys: string[];
  contextDocumentIds: string[];
};

export type ProjectContextGraphSpoke = {
  id: string;
  edgeKey: string;
  coordinateKey: string;
  sourceId: string;
  targetId: string;
};

export type ProjectContextGraphIsland = {
  stableKey: string;
  index: number;
  coordinateKeys: string[];
  edgeKeys: string[];
  contextDocumentIds: string[];
};

export type ProjectContextGraphModel = {
  anchorCoordinateKeys: string[];
  coordinates: ProjectContextGraphCoordinate[];
  hubs: ProjectContextGraphHub[];
  isAllContext: boolean;
  spokes: ProjectContextGraphSpoke[];
  islands: ProjectContextGraphIsland[];
};

const OBJECT_TYPE_LABELS: Record<string, string> = {
  project_profile: "Project profile",
  goal: "Goal",
  role: "Role",
  plan: "Plan",
  stage: "Stage",
  requirement: "Requirement",
  issue: "Issue",
  work: "Work",
  resource: "Resource",
};

function compareText(left: string, right: string) {
  return left.localeCompare(right, "en");
}

/** Stable React Flow node id for one real Project Context Coordinate. */
export function projectContextCoordinateNodeId(coordinateKey: string): string {
  return `coordinate:${coordinateKey}`;
}

/** Stable React Flow node id for one domain Context Edge hub. */
export function projectContextHubNodeId(edgeKey: string): string {
  return `edge-hub:${edgeKey}`;
}

/** Stable React Flow edge id for one incidence Spoke. */
export function projectContextSpokeId(
  edgeKey: string,
  coordinateKey: string,
): string {
  return `spoke:${edgeKey}:${coordinateKey}`;
}

function stableIdForDetail(
  coordinateKey: string,
  detail?: ProjectContextCoordinateDetail,
): string {
  if (detail?.coordinate.type === "document") {
    return detail.coordinate.documentId;
  }
  if (detail?.coordinate.type === "project_view_object") {
    return detail.coordinate.objectId;
  }
  return coordinateKey.includes(":")
    ? coordinateKey.slice(coordinateKey.indexOf(":") + 1)
    : coordinateKey;
}

function shortStableId(stableId: string) {
  return stableId.length > 12
    ? `${stableId.slice(0, 8)}…${stableId.slice(-4)}`
    : stableId;
}

function typeLabelForDetail(detail?: ProjectContextCoordinateDetail): string {
  if (detail?.coordinate.type === "document") return "Document";
  if (detail?.coordinate.type === "project_view_object") {
    return (
      OBJECT_TYPE_LABELS[detail.coordinate.objectType] ??
      detail.coordinate.objectType
    );
  }
  return "Coordinate";
}

function coordinatePresentation(
  coordinateKey: string,
  detail?: ProjectContextCoordinateDetail,
): ProjectContextGraphCoordinate {
  const stableId = stableIdForDetail(coordinateKey, detail);
  const typeLabel = typeLabelForDetail(detail);
  return {
    id: projectContextCoordinateNodeId(coordinateKey),
    coordinateKey,
    coordinate: detail?.coordinate,
    displayTitle:
      detail?.title?.trim() || `${typeLabel} ${shortStableId(stableId)}`,
    stableId,
    state: detail?.state ?? "unavailable",
    typeLabel,
    objectType:
      detail?.coordinate.type === "project_view_object"
        ? detail.coordinate.objectType
        : undefined,
    unavailableReason: detail?.unavailableReason,
  };
}

function deriveIslands(
  hubs: ProjectContextGraphHub[],
): ProjectContextGraphIsland[] {
  const hubsByKey = new Map(hubs.map((hub) => [hub.edgeKey, hub]));
  const unvisited = new Set(hubs.map((hub) => hub.edgeKey));
  const edgeKeysByCoordinate = new Map<string, string[]>();

  for (const hub of hubs) {
    for (const coordinateKey of hub.coordinateKeys) {
      const incident = edgeKeysByCoordinate.get(coordinateKey) ?? [];
      incident.push(hub.edgeKey);
      edgeKeysByCoordinate.set(coordinateKey, incident);
    }
  }

  const components: Omit<ProjectContextGraphIsland, "index">[] = [];
  while (unvisited.size > 0) {
    const seed = [...unvisited].sort(compareText)[0];
    const queue = [seed];
    const edgeKeys = new Set<string>();
    const coordinateKeys = new Set<string>();
    const contextDocumentIds = new Set<string>();
    unvisited.delete(seed);

    while (queue.length > 0) {
      const edgeKey = queue.shift();
      if (!edgeKey) continue;
      const hub = hubsByKey.get(edgeKey);
      if (!hub) continue;
      edgeKeys.add(edgeKey);
      for (const documentId of hub.contextDocumentIds) {
        contextDocumentIds.add(documentId);
      }
      for (const coordinateKey of hub.coordinateKeys) {
        coordinateKeys.add(coordinateKey);
        for (const incidentEdgeKey of edgeKeysByCoordinate.get(coordinateKey) ??
          []) {
          if (unvisited.delete(incidentEdgeKey)) {
            queue.push(incidentEdgeKey);
          }
        }
      }
    }

    const sortedEdgeKeys = [...edgeKeys].sort(compareText);
    components.push({
      stableKey: sortedEdgeKeys.join("|"),
      edgeKeys: sortedEdgeKeys,
      coordinateKeys: [...coordinateKeys].sort(compareText),
      contextDocumentIds: [...contextDocumentIds].sort(compareText),
    });
  }

  return components
    .sort((left, right) => compareText(left.stableKey, right.stableKey))
    .map((component, index) => ({ ...component, index: index + 1 }));
}

/**
 * Convert one trusted query result into a canonical incidence graph.
 * Context Document bindings are facts on hubs and never create implicit nodes.
 */
export function buildProjectContextGraph(
  result: ProjectContextQueryResult,
): ProjectContextGraphModel {
  const detailByKey = new Map(
    result.coordinateDetails.map((detail) => [detail.coordinateKey, detail]),
  );
  const queryCoordinates =
    result.query.type === "incident"
      ? [result.query.coordinate]
      : result.query.coordinates;
  const anchorCoordinateKeys = queryCoordinates
    .map(projectContextCoordinateKey)
    .sort(compareText);
  const coordinateKeys = new Set<string>(anchorCoordinateKeys);
  const hubs = [...result.edges]
    .sort((left, right) => compareText(left.edgeKey, right.edgeKey))
    .map((edge) => {
      const canonicalCoordinateKeys = [...new Set(edge.coordinateKeys)].sort(
        compareText,
      );
      const contextDocumentIds = [...new Set(edge.contextDocumentIds)].sort(
        compareText,
      );
      for (const coordinateKey of canonicalCoordinateKeys) {
        coordinateKeys.add(coordinateKey);
      }
      return {
        id: projectContextHubNodeId(edge.edgeKey),
        edgeKey: edge.edgeKey,
        coordinateKeys: canonicalCoordinateKeys,
        contextDocumentIds,
      };
    });

  const coordinates = [...coordinateKeys]
    .sort(compareText)
    .map((coordinateKey) =>
      coordinatePresentation(coordinateKey, detailByKey.get(coordinateKey)),
    );
  const spokes = hubs.flatMap((hub) =>
    hub.coordinateKeys.map((coordinateKey) => ({
      id: projectContextSpokeId(hub.edgeKey, coordinateKey),
      edgeKey: hub.edgeKey,
      coordinateKey,
      sourceId: hub.id,
      targetId: projectContextCoordinateNodeId(coordinateKey),
    })),
  );

  return {
    anchorCoordinateKeys,
    coordinates,
    hubs,
    isAllContext:
      result.query.type === "contains_all" &&
      result.query.coordinates.length === 0,
    spokes,
    islands: deriveIslands(hubs),
  };
}
