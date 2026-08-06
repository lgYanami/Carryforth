import {
  indexProjectViewObjects,
  projectViewIncomingReferences,
} from "@/features/project-view/model";
import type {
  ProjectContextCoordinateDetail,
  ProjectContextDocumentDetail,
  ProjectContextEdge,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import { projectContextCoordinateFromKey } from "@/shared/api/tauriProjectContext";
import type { ProjectDocumentIdentity } from "@/shared/api/tauriProjectDocument";
import type {
  ProjectView,
  ProjectViewLoadResult,
  ProjectViewObject,
} from "@/shared/api/tauriProjectView";

export type ProjectContextInspectedEdge = {
  edge: ProjectContextEdge;
  coordinates: ProjectContextCoordinateDetail[];
  documents: ProjectContextDocumentDetail[];
};

export type ProjectContextProjectViewRelation = {
  direction: "incoming" | "outgoing";
  label: string;
  target: ProjectViewObject;
};

function unavailableCoordinateDetail(
  coordinateKey: string,
): ProjectContextCoordinateDetail {
  return {
    coordinateKey,
    coordinate: projectContextCoordinateFromKey(coordinateKey),
    state: "unavailable",
    unavailableReason:
      "Current Coordinate details are unavailable, but its verified Edge membership remains visible.",
  };
}

function unavailableDocumentDetail(
  documentId: string,
): ProjectContextDocumentDetail {
  return {
    documentId,
    state: "unavailable",
    unavailableReason:
      "Current Document details are unavailable, but its verified Context binding remains visible.",
  };
}

/** Resolve one Coordinate without removing verified topology when hydration is absent. */
export function projectContextInspectedCoordinate(
  result: ProjectContextQueryResult,
  coordinateKey: string,
): ProjectContextCoordinateDetail {
  return (
    result.coordinateDetails.find(
      (detail) => detail.coordinateKey === coordinateKey,
    ) ?? unavailableCoordinateDetail(coordinateKey)
  );
}

/** Stable current-result Edge memberships for one Coordinate. */
export function projectContextIncidentEdgeKeys(
  result: ProjectContextQueryResult,
  coordinateKey: string,
): string[] {
  return result.edges
    .filter((edge) => edge.coordinateKeys.includes(coordinateKey))
    .map((edge) => edge.edgeKey)
    .sort((left, right) => left.localeCompare(right, "en"));
}

/** Resolve one complete Edge, including unavailable Coordinate/Document rows. */
export function projectContextInspectedEdge(
  result: ProjectContextQueryResult,
  edgeKey: string,
): ProjectContextInspectedEdge | undefined {
  const edge = result.edges.find((candidate) => candidate.edgeKey === edgeKey);
  if (!edge) return undefined;
  const documentById = new Map(
    result.documentDetails.map((detail) => [detail.documentId, detail]),
  );
  return {
    edge,
    coordinates: [...edge.coordinateKeys]
      .sort((left, right) => left.localeCompare(right, "en"))
      .map((coordinateKey) =>
        projectContextInspectedCoordinate(result, coordinateKey),
      ),
    documents: [...edge.contextDocumentIds]
      .sort((left, right) => left.localeCompare(right, "en"))
      .map(
        (documentId) =>
          documentById.get(documentId) ?? unavailableDocumentDetail(documentId),
      ),
  };
}

/** Identity authorized by the Document observation embedded in this Context result. */
export function projectContextDocumentIdentity(
  result: ProjectContextQueryResult,
): ProjectDocumentIdentity | undefined {
  const observation = result.documentObservation;
  if (
    observation.state !== "observed" ||
    observation.projectionGeneration === undefined
  ) {
    return undefined;
  }
  return {
    communityKey: result.communityKey,
    projectId: result.projectId,
    relayPubkey: result.relayPubkey,
    projectionGeneration: observation.projectionGeneration,
  };
}

/** First stable, active Context Document that may be read through this result. */
export function firstReadableProjectContextDocumentId(
  documents: ProjectContextDocumentDetail[],
  identity?: ProjectDocumentIdentity,
): string | undefined {
  if (!identity) return undefined;
  return documents.find((document) => document.state === "active")?.documentId;
}

/** Resolve an active object only from the matching current verified View source. */
export function projectContextProjectViewObject(input: {
  detail: ProjectContextCoordinateDetail;
  projectViewResult?: ProjectViewLoadResult;
  result: ProjectContextQueryResult;
}): ProjectViewObject | undefined {
  const { detail, projectViewResult, result } = input;
  if (
    detail.state !== "active" ||
    detail.coordinate.type !== "project_view_object" ||
    projectViewResult?.status !== "ready" ||
    result.projectViewObservation.state !== "observed" ||
    result.projectViewObservation.projectionGeneration === undefined ||
    projectViewResult.relayPubkey !== result.relayPubkey ||
    projectViewResult.projectionGeneration !==
      result.projectViewObservation.projectionGeneration
  ) {
    return undefined;
  }
  const object = indexProjectViewObjects(projectViewResult.view).get(
    detail.coordinate.objectId,
  );
  return object?.objectType === detail.coordinate.objectType
    ? object
    : undefined;
}

/** Outgoing and incoming direct Project View relations for a compact inspector. */
export function projectContextProjectViewRelations(
  view: ProjectView,
  object: ProjectViewObject,
): ProjectContextProjectViewRelation[] {
  const objects = indexProjectViewObjects(view);
  const outgoing = [
    object.relations.underGoalId
      ? { label: "Under goal", objectId: object.relations.underGoalId }
      : undefined,
    object.relations.underPlanId
      ? { label: "Under plan", objectId: object.relations.underPlanId }
      : undefined,
    object.relations.plannedInStageId
      ? {
          label: "Planned in stage",
          objectId: object.relations.plannedInStageId,
        }
      : undefined,
    object.relations.about
      ? { label: "About", objectId: object.relations.about.objectId }
      : undefined,
    object.relations.handles
      ? { label: "Handles", objectId: object.relations.handles.objectId }
      : undefined,
  ].flatMap((relation) => {
    if (!relation) return [];
    const target = objects.get(relation.objectId);
    return target
      ? [{ direction: "outgoing" as const, label: relation.label, target }]
      : [];
  });
  const incoming = projectViewIncomingReferences(view, object.id).map(
    (reference) => ({
      direction: "incoming" as const,
      label: reference.relation,
      target: reference.source,
    }),
  );
  return [...outgoing, ...incoming].sort(
    (left, right) =>
      left.direction.localeCompare(right.direction, "en") ||
      left.label.localeCompare(right.label, "en") ||
      left.target.id.localeCompare(right.target.id, "en"),
  );
}
