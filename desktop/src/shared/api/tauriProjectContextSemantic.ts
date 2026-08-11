import { invokeTauri, TauriInvokeError } from "@/shared/api/tauri";
import {
  canonicalizeProjectContextCoordinates,
  ProjectContextError,
  type ProjectContextCoordinate,
} from "@/shared/api/tauriProjectContext";

export const SEMANTIC_PROJECT_CONTEXT_MAX_PROBLEM_BYTES = 16 * 1024;
export const SEMANTIC_PROJECT_CONTEXT_MAX_INITIAL_COORDINATES = 16;
export const SEMANTIC_PROJECT_CONTEXT_MAX_CONTEXT_COORDINATES = 8;

export type SemanticProjectContextQueryInput = {
  communityKey: string;
  appliedWorkspaceToken: string;
  problem: string;
  initialCoordinates: ProjectContextCoordinate[];
  contextCoordinates: ProjectContextCoordinate[];
};

/** Current trusted identity against which a native response may be accepted. */
export type SemanticProjectContextAcceptanceIdentity = {
  communityKey: string;
  appliedWorkspaceToken: string;
  callerPubkey: string;
  projectId: string;
  relayPubkey: string;
};

export type SemanticProjectContextCompletionReason =
  | "frontier_exhausted"
  | "budget_exhausted"
  | "wall_time_exhausted";

export type SemanticProjectContextExhaustedDimension =
  | "recall_per_channel"
  | "semantic_roots"
  | "hops_per_path"
  | "beam_width"
  | "expanded_coordinates"
  | "incident_edges_materialized"
  | "relation_options_materialized"
  | "target_options_materialized"
  | "paths"
  | "response_bytes";

export type SemanticProjectContextBranchStopReason =
  | "frontier_exhausted"
  | "below_relevance_threshold"
  | "cycle_or_duplicate"
  | "max_hops_reached"
  | "hyperedge_too_large"
  | "global_budget_exhausted"
  | "wall_time_exhausted";

export type SemanticProjectContextInitialOmissionReason =
  | "source_not_found"
  | "source_deleted"
  | "source_tombstoned"
  | "source_ineligible";

export type SemanticProjectContextContextOmissionReason =
  | "source_not_found"
  | "source_ineligible"
  | "semantic_head_missing"
  | "semantic_head_building"
  | "semantic_head_failed"
  | "conditioned_input_unsupported";

export type SemanticProjectContextInitialOutcome =
  | { coordinateKey: string; state: "accepted" }
  | { coordinateKey: string; state: "not_in_graph" }
  | {
      coordinateKey: string;
      state: "omitted";
      reason: SemanticProjectContextInitialOmissionReason;
    };

export type SemanticProjectContextContextOutcome =
  | { coordinateKey: string; state: "accepted" }
  | {
      coordinateKey: string;
      state: "omitted";
      reason: SemanticProjectContextContextOmissionReason;
    };

export type SemanticProjectContextCoverage = {
  authorizedGraphSources: number;
  currentIndexedGraphSources: number;
  titleOnlySources: number;
  rootsReturned: number;
  pathsReturned: number;
  omittedInitialCoordinates: number;
  omittedContextCoordinates: number;
  indexCoveragePartial: number;
  omittedForResponseBudget: {
    automaticRoots: number;
    paths: number;
    summaries: number;
  };
};

export type SemanticProjectContextRoot = {
  rootId: string;
  coordinateEntrypoints: string[];
  contextDocumentEntrypoints: Array<{
    edgeKey: string;
    documentId: string;
  }>;
};

export type SemanticProjectContextPathHop = {
  ordinal: number;
  edgeKey: string;
  completeCoordinateKeys: string[];
  currentContextDocumentIds: string[];
  enteredFromCoordinateKey?: string;
  selectedContextDocumentId: string;
  continuedToCoordinateKey: string;
};

export type SemanticProjectContextPath = {
  pathId: string;
  rootId: string;
  branchStopReason: SemanticProjectContextBranchStopReason;
  hops: SemanticProjectContextPathHop[];
};

/** Verified, body-free display DTO returned by the trusted native boundary. */
export type SemanticProjectContextQueryResult = {
  communityKey: string;
  appliedWorkspaceToken: string;
  callerPubkey: string;
  requestId: string;
  projectId: string;
  relayPubkey: string;
  projectContextRevision: number;
  snapshotObservedAt: string;
  completionReason: SemanticProjectContextCompletionReason;
  exhaustedDimensions: SemanticProjectContextExhaustedDimension[];
  coverage: SemanticProjectContextCoverage;
  inputOutcomes: {
    initial: SemanticProjectContextInitialOutcome[];
    context: SemanticProjectContextContextOutcome[];
  };
  roots: SemanticProjectContextRoot[];
  paths: SemanticProjectContextPath[];
};

export type SemanticProjectContextErrorCode =
  | "invalid_input"
  | "unsupported"
  | "restricted"
  | "busy"
  | "conflict"
  | "timeout"
  | "too_large"
  | "unavailable"
  | "verification_failed"
  | "internal";

export type SemanticProjectContextErrorPayload = {
  code: SemanticProjectContextErrorCode;
  message: string;
  status?: number;
  retryable: boolean;
  retryAfterSeconds?: number;
};

const ERROR_CODES = new Set<SemanticProjectContextErrorCode>([
  "invalid_input",
  "unsupported",
  "restricted",
  "busy",
  "conflict",
  "timeout",
  "too_large",
  "unavailable",
  "verification_failed",
  "internal",
]);

export class SemanticProjectContextError extends Error {
  readonly code: SemanticProjectContextErrorCode;
  readonly status?: number;
  readonly retryable: boolean;
  readonly retryAfterSeconds?: number;

  constructor(payload: SemanticProjectContextErrorPayload) {
    super(payload.message);
    this.name = "SemanticProjectContextError";
    this.code = payload.code;
    this.status = payload.status;
    this.retryable = payload.retryable;
    this.retryAfterSeconds = payload.retryAfterSeconds;
  }
}

function invalidInput(message: string): SemanticProjectContextError {
  return new SemanticProjectContextError({
    code: "invalid_input",
    message,
    retryable: false,
  });
}

function requireOpaqueIdentity(value: string, field: string): string {
  if (value.length === 0 || value.includes("\0")) {
    throw invalidInput(`${field} must be a non-empty opaque identity.`);
  }
  return value;
}

/** Return the UTF-8 byte count used by both the form and native contract. */
export function semanticProjectContextProblemBytes(problem: string): number {
  return new TextEncoder().encode(problem.trim()).byteLength;
}

/** Validate and return the exact canonical problem sent to native. */
export function canonicalizeSemanticProjectContextProblem(
  problem: string,
): string {
  const canonical = problem.trim();
  if (canonical.length === 0) {
    throw invalidInput("Problem must not be blank.");
  }
  if (canonical.includes("\0")) {
    throw invalidInput("Problem must not contain NUL.");
  }
  const byteLength = new TextEncoder().encode(canonical).byteLength;
  if (byteLength > SEMANTIC_PROJECT_CONTEXT_MAX_PROBLEM_BYTES) {
    throw invalidInput(
      `Problem must not exceed ${SEMANTIC_PROJECT_CONTEXT_MAX_PROBLEM_BYTES} UTF-8 bytes.`,
    );
  }
  return canonical;
}

function canonicalizeCoordinateGroup(
  coordinates: ProjectContextCoordinate[],
  maximum: number,
  label: string,
): ProjectContextCoordinate[] {
  if (coordinates.length > maximum) {
    throw invalidInput(`${label} accepts at most ${maximum} Coordinates.`);
  }
  try {
    return canonicalizeProjectContextCoordinates(coordinates);
  } catch (error) {
    if (error instanceof ProjectContextError) {
      throw invalidInput(error.message);
    }
    throw error;
  }
}

/** Validate, deduplicate, and stably order one semantic Coordinate role. */
export function canonicalizeSemanticProjectContextCoordinates(
  coordinates: ProjectContextCoordinate[],
  role: "initial" | "context",
): ProjectContextCoordinate[] {
  return canonicalizeCoordinateGroup(
    coordinates,
    role === "initial"
      ? SEMANTIC_PROJECT_CONTEXT_MAX_INITIAL_COORDINATES
      : SEMANTIC_PROJECT_CONTEXT_MAX_CONTEXT_COORDINATES,
    role === "initial" ? "Initial Coordinates" : "Context Coordinates",
  );
}

/**
 * Construct the closed native input. Unknown UI fields (including lifecycle
 * and budget) are deliberately not copied across this boundary.
 */
export function canonicalizeSemanticProjectContextQueryInput(
  input: SemanticProjectContextQueryInput,
): SemanticProjectContextQueryInput {
  return {
    communityKey: requireOpaqueIdentity(input.communityKey, "communityKey"),
    appliedWorkspaceToken: requireOpaqueIdentity(
      input.appliedWorkspaceToken,
      "appliedWorkspaceToken",
    ),
    problem: canonicalizeSemanticProjectContextProblem(input.problem),
    initialCoordinates: canonicalizeSemanticProjectContextCoordinates(
      input.initialCoordinates,
      "initial",
    ),
    contextCoordinates: canonicalizeSemanticProjectContextCoordinates(
      input.contextCoordinates,
      "context",
    ),
  };
}

function isErrorPayload(
  value: unknown,
): value is SemanticProjectContextErrorPayload {
  if (typeof value !== "object" || value === null) return false;
  const payload = value as Record<string, unknown>;
  return (
    typeof payload.code === "string" &&
    ERROR_CODES.has(payload.code as SemanticProjectContextErrorCode) &&
    typeof payload.message === "string" &&
    typeof payload.retryable === "boolean" &&
    (payload.status === undefined || typeof payload.status === "number") &&
    (payload.retryAfterSeconds === undefined ||
      typeof payload.retryAfterSeconds === "number")
  );
}

/** Convert only the closed native semantic error vocabulary. */
export function semanticProjectContextErrorFromPayload(
  value: unknown,
): SemanticProjectContextError | undefined {
  return isErrorPayload(value)
    ? new SemanticProjectContextError(value)
    : undefined;
}

function identityMismatch(message: string): SemanticProjectContextError {
  return new SemanticProjectContextError({
    code: "conflict",
    message,
    retryable: true,
  });
}

/** Fail closed unless every trusted response identity is still current. */
export function requireMatchingSemanticProjectContextResponse(
  expected: SemanticProjectContextAcceptanceIdentity,
  result: SemanticProjectContextQueryResult,
): SemanticProjectContextQueryResult {
  if (
    result.communityKey !== expected.communityKey ||
    result.appliedWorkspaceToken !== expected.appliedWorkspaceToken
  ) {
    throw new SemanticProjectContextError({
      code: "verification_failed",
      message:
        "Semantic paths were returned for a different applied Community workspace.",
      retryable: false,
    });
  }
  if (result.callerPubkey !== expected.callerPubkey) {
    throw new SemanticProjectContextError({
      code: "verification_failed",
      message: "Semantic paths were returned for a different caller identity.",
      retryable: false,
    });
  }
  if (
    result.projectId !== expected.projectId ||
    result.relayPubkey !== expected.relayPubkey
  ) {
    throw new SemanticProjectContextError({
      code: "verification_failed",
      message: "Semantic paths were returned for a different Project or Relay.",
      retryable: false,
    });
  }
  return result;
}

/** Execute one trusted semantic query without exposing lifecycle or budget. */
export async function queryProjectContextSemantic(
  input: SemanticProjectContextQueryInput,
  expectedIdentity: SemanticProjectContextAcceptanceIdentity,
): Promise<SemanticProjectContextQueryResult> {
  const canonical = canonicalizeSemanticProjectContextQueryInput(input);
  if (
    canonical.communityKey !== expectedIdentity.communityKey ||
    canonical.appliedWorkspaceToken !== expectedIdentity.appliedWorkspaceToken
  ) {
    throw identityMismatch(
      "The semantic query does not belong to the current applied workspace.",
    );
  }
  try {
    const result = await invokeTauri<SemanticProjectContextQueryResult>(
      "query_project_context_semantic",
      { input: canonical },
    );
    return requireMatchingSemanticProjectContextResponse(
      expectedIdentity,
      result,
    );
  } catch (error) {
    if (error instanceof SemanticProjectContextError) throw error;
    const payload =
      error instanceof TauriInvokeError ? error.payload : (error as unknown);
    const mapped = semanticProjectContextErrorFromPayload(payload);
    if (mapped) throw mapped;
    throw error;
  }
}
