import {
  canonicalizeSemanticProjectContextCoordinates,
  canonicalizeSemanticProjectContextProblem,
  SEMANTIC_PROJECT_CONTEXT_MAX_CONTEXT_COORDINATES,
  SEMANTIC_PROJECT_CONTEXT_MAX_INITIAL_COORDINATES,
  semanticProjectContextProblemBytes,
  SemanticProjectContextError,
} from "@/shared/api/tauriProjectContextSemantic";
import {
  projectContextCoordinateKey,
  type ProjectContextCoordinate,
} from "@/shared/api/tauriProjectContext";

export type SemanticQueryDraft = {
  problem: string;
  initialCoordinates: ProjectContextCoordinate[];
  contextCoordinates: ProjectContextCoordinate[];
};

export type SubmittedSemanticQueryDraft = Readonly<{
  problem: string;
  initialCoordinates: ProjectContextCoordinate[];
  contextCoordinates: ProjectContextCoordinate[];
}>;

export type SemanticQueryCoordinateRole = "initial" | "context";

export type SemanticQueryDraftCoordinateRejection = "duplicate" | "limit";

export type SemanticQueryDraftCoordinateTransition =
  | { status: "changed"; draft: SemanticQueryDraft }
  | {
      status: "unchanged";
      draft: SemanticQueryDraft;
      reason: SemanticQueryDraftCoordinateRejection;
    };

export type SemanticQueryDraftValidation =
  | {
      valid: true;
      submitted: SubmittedSemanticQueryDraft;
      problemBytes: number;
    }
  | {
      valid: false;
      code: "invalid_input";
      message: string;
      problemBytes: number;
    };

/** Format a verified semantic path/root count with the correct English number. */
export function semanticGraphCountLabel(
  count: number,
  noun: "path" | "root",
): string {
  return `${count} ${count === 1 ? noun : `${noun}s`}`;
}

/** Start with an empty, Community-local in-memory semantic query draft. */
export function createSemanticQueryDraft(): SemanticQueryDraft {
  return {
    problem: "",
    initialCoordinates: [],
    contextCoordinates: [],
  };
}

/** Replace only the editable problem without affecting active query state. */
export function updateSemanticQueryDraftProblem(
  draft: SemanticQueryDraft,
  problem: string,
): SemanticQueryDraft {
  return { ...draft, problem };
}

function roleCoordinates(
  draft: SemanticQueryDraft,
  role: SemanticQueryCoordinateRole,
): ProjectContextCoordinate[] {
  return role === "initial"
    ? draft.initialCoordinates
    : draft.contextCoordinates;
}

function roleMaximum(role: SemanticQueryCoordinateRole): number {
  return role === "initial"
    ? SEMANTIC_PROJECT_CONTEXT_MAX_INITIAL_COORDINATES
    : SEMANTIC_PROJECT_CONTEXT_MAX_CONTEXT_COORDINATES;
}

function replaceRoleCoordinates(
  draft: SemanticQueryDraft,
  role: SemanticQueryCoordinateRole,
  coordinates: ProjectContextCoordinate[],
): SemanticQueryDraft {
  return role === "initial"
    ? { ...draft, initialCoordinates: coordinates }
    : { ...draft, contextCoordinates: coordinates };
}

/** Add one Coordinate idempotently within its role; cross-role reuse is valid. */
export function tryAddSemanticQueryDraftCoordinate(
  draft: SemanticQueryDraft,
  role: SemanticQueryCoordinateRole,
  coordinate: ProjectContextCoordinate,
): SemanticQueryDraftCoordinateTransition {
  const current = roleCoordinates(draft, role);
  const key = projectContextCoordinateKey(coordinate);
  if (
    current.some((candidate) => projectContextCoordinateKey(candidate) === key)
  ) {
    return { status: "unchanged", draft, reason: "duplicate" };
  }
  if (current.length >= roleMaximum(role)) {
    return { status: "unchanged", draft, reason: "limit" };
  }
  return {
    status: "changed",
    draft: replaceRoleCoordinates(
      draft,
      role,
      canonicalizeSemanticProjectContextCoordinates(
        [...current, coordinate],
        role,
      ),
    ),
  };
}

/** Add one Coordinate as a total, idempotent draft transition. */
export function addSemanticQueryDraftCoordinate(
  draft: SemanticQueryDraft,
  role: SemanticQueryCoordinateRole,
  coordinate: ProjectContextCoordinate,
): SemanticQueryDraft {
  return tryAddSemanticQueryDraftCoordinate(draft, role, coordinate).draft;
}

/** Remove one Coordinate from exactly one semantic role. */
export function removeSemanticQueryDraftCoordinate(
  draft: SemanticQueryDraft,
  role: SemanticQueryCoordinateRole,
  coordinateKey: string,
): SemanticQueryDraft {
  return replaceRoleCoordinates(
    draft,
    role,
    roleCoordinates(draft, role).filter(
      (coordinate) => projectContextCoordinateKey(coordinate) !== coordinateKey,
    ),
  );
}

/** Validate the form without throwing during React render. */
export function validateSemanticQueryDraft(
  draft: SemanticQueryDraft,
): SemanticQueryDraftValidation {
  const problemBytes = semanticProjectContextProblemBytes(draft.problem);
  try {
    return {
      valid: true,
      submitted: {
        problem: canonicalizeSemanticProjectContextProblem(draft.problem),
        initialCoordinates: canonicalizeSemanticProjectContextCoordinates(
          draft.initialCoordinates,
          "initial",
        ),
        contextCoordinates: canonicalizeSemanticProjectContextCoordinates(
          draft.contextCoordinates,
          "context",
        ),
      },
      problemBytes,
    };
  } catch (error) {
    return {
      valid: false,
      code: "invalid_input",
      message:
        error instanceof SemanticProjectContextError
          ? error.message
          : "Semantic query input is invalid.",
      problemBytes,
    };
  }
}

/** Return a canonical immutable submission or throw the closed input error. */
export function submitSemanticQueryDraft(
  draft: SemanticQueryDraft,
): SubmittedSemanticQueryDraft {
  const validation = validateSemanticQueryDraft(draft);
  if (!validation.valid) {
    throw new SemanticProjectContextError({
      code: validation.code,
      message: validation.message,
      retryable: false,
    });
  }
  return validation.submitted;
}

/** Compare editable input to the exact canonical draft behind an active run. */
export function semanticQueryDraftMatchesSubmission(
  draft: SemanticQueryDraft,
  submitted: SubmittedSemanticQueryDraft,
): boolean {
  const validation = validateSemanticQueryDraft(draft);
  return (
    validation.valid &&
    JSON.stringify(validation.submitted) === JSON.stringify(submitted)
  );
}
