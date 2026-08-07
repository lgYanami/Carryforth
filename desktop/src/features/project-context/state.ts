import {
  ProjectContextError,
  type ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";

export type ProjectContextFailureKind =
  | "unsupported"
  | "restricted"
  | "unavailable"
  | "snapshot_conflict"
  | "verification_failed"
  | "error";

export function projectContextFailureKind(
  error: unknown,
): ProjectContextFailureKind {
  if (!(error instanceof ProjectContextError)) return "error";
  switch (error.code) {
    case "unsupported":
    case "restricted":
    case "unavailable":
    case "snapshot_conflict":
    case "verification_failed":
      return error.code;
    case "invalid_input":
    case "internal":
      return "error";
  }
}

export function projectContextErrorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : "Desktop could not read the verified Project Context projection.";
}

export function visibleContextDocumentCount(
  result: ProjectContextQueryResult,
): number {
  return new Set(result.edges.flatMap((edge) => edge.contextDocumentIds)).size;
}
