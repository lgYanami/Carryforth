import type {
  SemanticProjectContextAcceptanceIdentity,
  SemanticProjectContextQueryResult,
} from "@/shared/api/tauriProjectContextSemantic";
import { SemanticProjectContextError } from "@/shared/api/tauriProjectContextSemantic";
import {
  createSemanticQueryDraft,
  type SemanticQueryDraft,
  type SubmittedSemanticQueryDraft,
} from "@/features/project-context/semanticQueryModel";

export type SemanticFreshness = {
  topology: "matched" | "advanced";
  sources: "no_change_observed" | "change_observed";
  transport: "live" | "uncertain";
};

export type SemanticSession<TOverlay = unknown> = {
  localAttemptToken: number;
  requestId: string;
  submittedDraft: SubmittedSemanticQueryDraft;
  verifiedDisplayResult: SemanticProjectContextQueryResult;
  overlay: TOverlay;
  projectContextRevision: number;
  snapshotObservedAt: string;
  communityKey: string;
  appliedWorkspaceToken: string;
  callerPubkey: string;
  relayPubkey: string;
  projectId: string;
};

export type SemanticAttempt =
  | { status: "idle" }
  | {
      status: "running";
      token: number;
      submitted: SubmittedSemanticQueryDraft;
    }
  | {
      status: "pairing";
      token: number;
      submitted: SubmittedSemanticQueryDraft;
      verifiedDisplayResult: SemanticProjectContextQueryResult;
    }
  | {
      status: "failed";
      token: number;
      error: SemanticProjectContextError;
    };

export type SemanticUiState<TOverlay = unknown> = {
  draft: SemanticQueryDraft;
  attempt: SemanticAttempt;
  active: SemanticSession<TOverlay> | null;
  freshness: SemanticFreshness;
  identity: SemanticProjectContextAcceptanceIdentity;
  /** Monotonic local fence for invoke and pairing responses. */
  generation: number;
};

export type SemanticPairingJoin<TOverlay> =
  | { status: "valid"; overlay: TOverlay }
  | { status: "invalid"; message?: string };

export type SemanticSessionAction<TOverlay = unknown> =
  | { type: "draft_changed"; draft: SemanticQueryDraft }
  | {
      type: "run_started";
      token: number;
      submitted: SubmittedSemanticQueryDraft;
    }
  | {
      type: "native_succeeded";
      token: number;
      result: SemanticProjectContextQueryResult;
    }
  | {
      type: "native_failed";
      token: number;
      error: SemanticProjectContextError;
    }
  | {
      type: "pairing_observed";
      token: number;
      substrateRevision: number;
      join: SemanticPairingJoin<TOverlay>;
    }
  | { type: "cancel" }
  | {
      type: "boundary_changed";
      identity: SemanticProjectContextAcceptanceIdentity;
    }
  | { type: "capability_lost" }
  | { type: "topology_observed"; substrateRevision: number }
  | { type: "source_hint" }
  | { type: "source_refresh_observed"; fingerprintMatches: boolean }
  | { type: "transport_observed"; state: "live" | "uncertain" };

const FRESH: SemanticFreshness = {
  topology: "matched",
  sources: "no_change_observed",
  transport: "live",
};

function fresh(): SemanticFreshness {
  return { ...FRESH };
}

function sameIdentity(
  left: SemanticProjectContextAcceptanceIdentity,
  right: SemanticProjectContextAcceptanceIdentity,
): boolean {
  return (
    left.communityKey === right.communityKey &&
    left.appliedWorkspaceToken === right.appliedWorkspaceToken &&
    left.callerPubkey === right.callerPubkey &&
    left.projectId === right.projectId &&
    left.relayPubkey === right.relayPubkey
  );
}

/** Check a verified DTO against the currently applied acceptance boundary. */
export function semanticResultMatchesIdentity(
  result: SemanticProjectContextQueryResult,
  identity: SemanticProjectContextAcceptanceIdentity,
): boolean {
  return (
    result.communityKey === identity.communityKey &&
    result.appliedWorkspaceToken === identity.appliedWorkspaceToken &&
    result.callerPubkey === identity.callerPubkey &&
    result.projectId === identity.projectId &&
    result.relayPubkey === identity.relayPubkey
  );
}

function conflict(message: string): SemanticProjectContextError {
  return new SemanticProjectContextError({
    code: "conflict",
    message,
    retryable: true,
  });
}

function verificationFailure(message: string): SemanticProjectContextError {
  return new SemanticProjectContextError({
    code: "verification_failed",
    message,
    retryable: false,
  });
}

/** Errors that invalidate trust in a previously displayed semantic snapshot. */
export function semanticFailureClearsActive(
  error: SemanticProjectContextError,
): boolean {
  return (
    error.code === "restricted" ||
    error.code === "unsupported" ||
    error.code === "verification_failed" ||
    error.code === "internal"
  );
}

/** Create state scoped to one already-applied native workspace identity. */
export function createSemanticUiState<TOverlay = unknown>(
  identity: SemanticProjectContextAcceptanceIdentity,
  draft: SemanticQueryDraft = createSemanticQueryDraft(),
): SemanticUiState<TOverlay> {
  return {
    draft,
    attempt: { status: "idle" },
    active: null,
    freshness: fresh(),
    identity,
    generation: 0,
  };
}

/** Allocate the next local token before dispatching `run_started`. */
export function nextSemanticAttemptToken(
  state: Pick<SemanticUiState, "generation">,
): number {
  return state.generation + 1;
}

function isCurrentAttempt(
  state: SemanticUiState,
  token: number,
  status?: SemanticAttempt["status"],
): boolean {
  return (
    token === state.generation &&
    state.attempt.status !== "idle" &&
    state.attempt.token === token &&
    (status === undefined || state.attempt.status === status)
  );
}

/**
 * Pure semantic session reducer. Token and identity checks make late Native
 * responses and late graph-pairing completions inert.
 */
export function semanticSessionReducer<TOverlay>(
  state: SemanticUiState<TOverlay>,
  action: SemanticSessionAction<TOverlay>,
): SemanticUiState<TOverlay> {
  switch (action.type) {
    case "draft_changed":
      return { ...state, draft: action.draft };
    case "run_started":
      if (action.token <= state.generation) return state;
      return {
        ...state,
        generation: action.token,
        attempt: {
          status: "running",
          token: action.token,
          submitted: action.submitted,
        },
      };
    case "native_succeeded": {
      if (!isCurrentAttempt(state, action.token, "running")) return state;
      const running = state.attempt;
      if (running.status !== "running") return state;
      if (!semanticResultMatchesIdentity(action.result, state.identity)) {
        return {
          ...state,
          attempt: {
            status: "failed",
            token: action.token,
            error: verificationFailure(
              "Semantic response identity no longer matches the applied workspace.",
            ),
          },
          active: null,
          freshness: fresh(),
        };
      }
      return {
        ...state,
        attempt: {
          status: "pairing",
          token: action.token,
          submitted: running.submitted,
          verifiedDisplayResult: action.result,
        },
      };
    }
    case "native_failed":
      if (!isCurrentAttempt(state, action.token)) return state;
      return {
        ...state,
        attempt: {
          status: "failed",
          token: action.token,
          error: action.error,
        },
        active: semanticFailureClearsActive(action.error) ? null : state.active,
        freshness: semanticFailureClearsActive(action.error)
          ? fresh()
          : state.freshness,
      };
    case "pairing_observed": {
      if (!isCurrentAttempt(state, action.token, "pairing")) return state;
      const pairing = state.attempt;
      if (pairing.status !== "pairing") return state;
      const resultRevision =
        pairing.verifiedDisplayResult.projectContextRevision;
      if (action.substrateRevision < resultRevision) return state;
      if (
        action.substrateRevision > resultRevision ||
        action.join.status === "invalid"
      ) {
        return {
          ...state,
          attempt: {
            status: "failed",
            token: action.token,
            error: conflict(
              action.join.status === "invalid" && action.join.message
                ? action.join.message
                : "Project Context advanced before semantic paths could be paired.",
            ),
          },
        };
      }
      const result = pairing.verifiedDisplayResult;
      return {
        ...state,
        attempt: { status: "idle" },
        active: {
          localAttemptToken: action.token,
          requestId: result.requestId,
          submittedDraft: pairing.submitted,
          verifiedDisplayResult: result,
          overlay: action.join.overlay,
          projectContextRevision: result.projectContextRevision,
          snapshotObservedAt: result.snapshotObservedAt,
          communityKey: result.communityKey,
          appliedWorkspaceToken: result.appliedWorkspaceToken,
          callerPubkey: result.callerPubkey,
          relayPubkey: result.relayPubkey,
          projectId: result.projectId,
        },
        freshness: fresh(),
      };
    }
    case "cancel":
      return {
        ...state,
        generation: state.generation + 1,
        attempt: { status: "idle" },
        active: null,
        freshness: fresh(),
      };
    case "boundary_changed":
      if (sameIdentity(state.identity, action.identity)) return state;
      return {
        ...state,
        draft: createSemanticQueryDraft(),
        identity: action.identity,
        generation: state.generation + 1,
        attempt: { status: "idle" },
        active: null,
        freshness: fresh(),
      };
    case "capability_lost":
      return {
        ...state,
        generation: state.generation + 1,
        attempt: { status: "idle" },
        active: null,
        freshness: fresh(),
      };
    case "topology_observed":
      if (
        !state.active ||
        action.substrateRevision === state.active.projectContextRevision
      ) {
        return state;
      }
      return {
        ...state,
        freshness: { ...state.freshness, topology: "advanced" },
      };
    case "source_hint":
      // Untrusted hints only schedule canonical refresh outside this reducer.
      return state;
    case "source_refresh_observed":
      if (!state.active || action.fingerprintMatches) return state;
      return {
        ...state,
        freshness: {
          ...state.freshness,
          sources: "change_observed",
        },
      };
    case "transport_observed":
      if (!state.active || state.freshness.transport === action.state) {
        return state;
      }
      return {
        ...state,
        freshness: { ...state.freshness, transport: action.state },
      };
  }
}

/** Whether the trusted All Context substrate is needed by current state. */
export function semanticQueryRequiresAllContext(
  state: SemanticUiState,
): boolean {
  return (
    state.active !== null ||
    state.attempt.status === "running" ||
    state.attempt.status === "pairing"
  );
}

/** Gate overlay rendering synchronously on the exact canonical graph revision. */
export function semanticOverlayEligible(
  state: SemanticUiState,
  canvasProjectContextRevision: number,
  atomicStructuralJoinIsValid = true,
): boolean {
  return (
    state.active !== null &&
    state.freshness.topology === "matched" &&
    state.active.projectContextRevision === canvasProjectContextRevision &&
    atomicStructuralJoinIsValid
  );
}

/** Closed DOM freshness vocabulary; topology mismatch also suspends the overlay. */
export function semanticSessionFreshness(
  state: SemanticUiState,
): "snapshot" | "stale" {
  return state.freshness.topology === "advanced" ||
    state.freshness.sources === "change_observed" ||
    state.freshness.transport === "uncertain"
    ? "stale"
    : "snapshot";
}
