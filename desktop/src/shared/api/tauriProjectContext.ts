import { invokeTauri, TauriInvokeError } from "@/shared/api/tauri";
import type { ProjectViewObjectType } from "@/shared/api/tauriProjectView";

export type ProjectContextCoordinate =
  | {
      type: "project_view_object";
      objectType: ProjectViewObjectType;
      objectId: string;
    }
  | {
      type: "document";
      documentId: string;
    };

export type ProjectContextQuery =
  | { type: "exact"; coordinates: ProjectContextCoordinate[] }
  | { type: "incident"; coordinate: ProjectContextCoordinate }
  | { type: "contains_all"; coordinates: ProjectContextCoordinate[] };

export type ProjectContextSourceState =
  | "not_requested"
  | "observed"
  | "unavailable";

export type ProjectContextDetailState = "active" | "tombstoned" | "unavailable";

export type ProjectContextObservation = {
  contextRevision: number;
  projectionGeneration: number;
  activeEdgeCount: number;
  boundDocumentCount: number;
  updatedAt: string;
  metaEventId: string;
  capabilityEnabled: boolean;
};

export type ProjectContextProjectViewObservation = {
  state: ProjectContextSourceState;
  projectRevision?: number;
  projectionGeneration?: number;
  updatedAt?: string;
  metaEventId?: string;
};

export type ProjectContextDocumentObservation = {
  state: ProjectContextSourceState;
  catalogRevision?: number;
  projectionGeneration?: number;
  updatedAt?: string;
  metaEventId?: string;
};

export type ProjectContextEdge = {
  edgeKey: string;
  coordinateKeys: string[];
  contextDocumentIds: string[];
};

export type ProjectContextCoordinateDetail = {
  coordinateKey: string;
  coordinate: ProjectContextCoordinate;
  state: ProjectContextDetailState;
  title?: string;
  status?: unknown;
  objectRevision?: number;
  documentRevision?: number;
  updatedAt?: string;
  updatedBy?: string;
  unavailableReason?: string;
};

export type ProjectContextDocumentDetail = {
  documentId: string;
  state: ProjectContextDetailState;
  title?: string;
  summary?: string;
  documentRevision?: number;
  updatedAt?: string;
  updatedBy?: string;
  unavailableReason?: string;
};

export type ProjectContextQueryResult = {
  communityKey: string;
  projectId: string;
  relayPubkey: string;
  context: ProjectContextObservation;
  query: ProjectContextQuery;
  projectViewObservation: ProjectContextProjectViewObservation;
  documentObservation: ProjectContextDocumentObservation;
  edges: ProjectContextEdge[];
  coordinateDetails: ProjectContextCoordinateDetail[];
  documentDetails: ProjectContextDocumentDetail[];
};

export type ProjectContextErrorCode =
  | "unsupported"
  | "restricted"
  | "unavailable"
  | "snapshot_conflict"
  | "invalid_input"
  | "verification_failed"
  | "internal";

export type ProjectContextErrorPayload = {
  code: ProjectContextErrorCode;
  message: string;
  status?: number;
  retryable: boolean;
  retryAfterSeconds?: number;
};

const ERROR_CODES = new Set<ProjectContextErrorCode>([
  "unsupported",
  "restricted",
  "unavailable",
  "snapshot_conflict",
  "invalid_input",
  "verification_failed",
  "internal",
]);

const OBJECT_TYPE_RANK: Record<ProjectViewObjectType, number> = {
  project_profile: 0,
  goal: 1,
  role: 2,
  plan: 3,
  stage: 4,
  requirement: 5,
  issue: 6,
  work: 7,
  resource: 8,
};

const UUID_V4_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export class ProjectContextError extends Error {
  readonly code: ProjectContextErrorCode;
  readonly status?: number;
  readonly retryable: boolean;
  readonly retryAfterSeconds?: number;

  constructor(payload: ProjectContextErrorPayload) {
    super(payload.message);
    this.name = "ProjectContextError";
    this.code = payload.code;
    this.status = payload.status;
    this.retryable = payload.retryable;
    this.retryAfterSeconds = payload.retryAfterSeconds;
  }
}

function canonicalUuid(value: string, field: string): string {
  const canonical = value.toLowerCase();
  if (!UUID_V4_PATTERN.test(canonical)) {
    throw new ProjectContextError({
      code: "invalid_input",
      message: `${field} must be a canonical UUID v4.`,
      retryable: false,
    });
  }
  return canonical;
}

function canonicalCoordinate(
  coordinate: ProjectContextCoordinate,
): ProjectContextCoordinate {
  if (coordinate.type === "document") {
    return {
      type: "document",
      documentId: canonicalUuid(coordinate.documentId, "documentId"),
    };
  }
  if (!(coordinate.objectType in OBJECT_TYPE_RANK)) {
    throw new ProjectContextError({
      code: "invalid_input",
      message: "objectType is not a Project View v3 object type.",
      retryable: false,
    });
  }
  return {
    type: "project_view_object",
    objectType: coordinate.objectType,
    objectId: canonicalUuid(coordinate.objectId, "objectId"),
  };
}

/** Stable coordinate token shared by query keys, URLs, and graph identities. */
export function projectContextCoordinateKey(
  coordinate: ProjectContextCoordinate,
): string {
  const canonical = canonicalCoordinate(coordinate);
  return canonical.type === "document"
    ? `document:${canonical.documentId}`
    : `${canonical.objectType}:${canonical.objectId}`;
}

function coordinateRank(coordinate: ProjectContextCoordinate): number {
  return coordinate.type === "document"
    ? 9
    : OBJECT_TYPE_RANK[coordinate.objectType];
}

/** Parse one canonical Coordinate token without accepting unknown object types. */
export function projectContextCoordinateFromKey(
  coordinateKey: string,
): ProjectContextCoordinate {
  const separator = coordinateKey.indexOf(":");
  if (separator <= 0 || separator === coordinateKey.length - 1) {
    throw new ProjectContextError({
      code: "invalid_input",
      message: "Project Context Coordinate token is malformed.",
      retryable: false,
    });
  }
  const type = coordinateKey.slice(0, separator);
  const id = coordinateKey.slice(separator + 1);
  if (type === "document") {
    return canonicalCoordinate({ type: "document", documentId: id });
  }
  if (!(type in OBJECT_TYPE_RANK)) {
    throw new ProjectContextError({
      code: "invalid_input",
      message: "Project Context Coordinate token has an unknown type.",
      retryable: false,
    });
  }
  return canonicalCoordinate({
    type: "project_view_object",
    objectType: type as ProjectViewObjectType,
    objectId: id,
  });
}

/** Validate, deduplicate, and stably order one Coordinate set. */
export function canonicalizeProjectContextCoordinates(
  coordinates: ProjectContextCoordinate[],
): ProjectContextCoordinate[] {
  const canonical = coordinates.map(canonicalCoordinate).sort((left, right) => {
    const rank = coordinateRank(left) - coordinateRank(right);
    return (
      rank ||
      projectContextCoordinateKey(left).localeCompare(
        projectContextCoordinateKey(right),
        "en",
      )
    );
  });
  const keys = canonical.map(projectContextCoordinateKey);
  if (keys.some((key, index) => index > 0 && key === keys[index - 1])) {
    throw new ProjectContextError({
      code: "invalid_input",
      message: "Project Context query coordinates must be distinct.",
      retryable: false,
    });
  }
  return canonical;
}

/** Validate and canonicalize one Desktop query before it becomes a cache key. */
export function canonicalizeProjectContextQuery(
  query: ProjectContextQuery,
): ProjectContextQuery {
  if (query.type === "incident") {
    return {
      type: "incident",
      coordinate: canonicalCoordinate(query.coordinate),
    };
  }
  const coordinates = canonicalizeProjectContextCoordinates(query.coordinates);
  if (query.type === "exact" && coordinates.length < 2) {
    throw new ProjectContextError({
      code: "invalid_input",
      message: "An exact Context Edge query requires at least two coordinates.",
      retryable: false,
    });
  }
  return { type: query.type, coordinates };
}

/** Stable serialized descriptor suitable for React Query keys. */
export function projectContextQueryKey(query: ProjectContextQuery): string {
  return JSON.stringify(canonicalizeProjectContextQuery(query));
}

function isErrorPayload(value: unknown): value is ProjectContextErrorPayload {
  if (typeof value !== "object" || value === null) return false;
  const payload = value as Record<string, unknown>;
  return (
    typeof payload.code === "string" &&
    ERROR_CODES.has(payload.code as ProjectContextErrorCode) &&
    typeof payload.message === "string" &&
    typeof payload.retryable === "boolean" &&
    (payload.status === undefined || typeof payload.status === "number") &&
    (payload.retryAfterSeconds === undefined ||
      typeof payload.retryAfterSeconds === "number")
  );
}

/** Convert one closed native payload without accepting arbitrary thrown objects. */
export function projectContextErrorFromPayload(
  value: unknown,
): ProjectContextError | undefined {
  return isErrorPayload(value) ? new ProjectContextError(value) : undefined;
}

function requireMatchingResponse(
  communityKey: string,
  query: ProjectContextQuery,
  result: ProjectContextQueryResult,
): ProjectContextQueryResult {
  if (result.communityKey !== communityKey) {
    throw new ProjectContextError({
      code: "snapshot_conflict",
      message:
        "The active Community changed while Project Context was loading.",
      retryable: true,
    });
  }
  let echoedQuery: ProjectContextQuery;
  try {
    echoedQuery = canonicalizeProjectContextQuery(result.query);
  } catch {
    throw new ProjectContextError({
      code: "verification_failed",
      message: "Desktop returned an invalid Project Context query descriptor.",
      retryable: false,
    });
  }
  if (JSON.stringify(echoedQuery) !== JSON.stringify(query)) {
    throw new ProjectContextError({
      code: "verification_failed",
      message: "Desktop returned a different Project Context query.",
      retryable: false,
    });
  }
  return result;
}

/** Execute one complete trusted Project Context query through Tauri. */
export async function queryProjectContext(input: {
  communityKey: string;
  query: ProjectContextQuery;
}): Promise<ProjectContextQueryResult> {
  const query = canonicalizeProjectContextQuery(input.query);
  try {
    const result = await invokeTauri<ProjectContextQueryResult>(
      "query_project_context",
      { input: { communityKey: input.communityKey, query } },
    );
    return requireMatchingResponse(input.communityKey, query, result);
  } catch (error) {
    if (error instanceof ProjectContextError) throw error;
    const payload =
      error instanceof TauriInvokeError ? error.payload : (error as unknown);
    const mapped = projectContextErrorFromPayload(payload);
    if (mapped) throw mapped;
    throw error;
  }
}
