import type { ProjectContextCoordinateOption } from "@/features/project-context/queryModel";
import type { ProjectContextSemanticOverlay } from "@/features/project-context/semanticOverlay";
import type { SemanticUiState } from "@/features/project-context/semanticSession";
import {
  projectContextCoordinateKey,
  ProjectContextError,
  type ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import { SemanticProjectContextError } from "@/shared/api/tauriProjectContextSemantic";

/** Convert closed structural/native failures without leaking arbitrary payloads. */
export function semanticErrorFromUnknown(
  error: unknown,
): SemanticProjectContextError {
  if (error instanceof SemanticProjectContextError) return error;
  if (error instanceof ProjectContextError) {
    const code =
      error.code === "snapshot_conflict"
        ? "conflict"
        : error.code === "verification_failed"
          ? "verification_failed"
          : error.code === "restricted"
            ? "restricted"
            : error.code === "unsupported"
              ? "unsupported"
              : error.code === "invalid_input"
                ? "invalid_input"
                : error.code === "internal"
                  ? "internal"
                  : "unavailable";
    return new SemanticProjectContextError({
      code,
      message: error.message,
      retryable:
        error.retryable &&
        code !== "verification_failed" &&
        code !== "internal",
      status: error.status,
      retryAfterSeconds: error.retryAfterSeconds,
    });
  }
  return new SemanticProjectContextError({
    code: "unavailable",
    message: "Semantic paths could not be loaded from the verified Relay.",
    retryable: true,
  });
}

/** Fingerprint canonical observations for only sources used by this session. */
export function semanticSourceFingerprint(
  active: NonNullable<SemanticUiState<ProjectContextSemanticOverlay>["active"]>,
  substrate: ProjectContextQueryResult,
  coordinateOptions: ProjectContextCoordinateOption[],
): string {
  const coordinateKeys = new Set<string>([
    ...active.overlay.memberCoordinateKeys,
    ...active.overlay.rootCoordinateKeys,
    ...active.submittedDraft.initialCoordinates.map(
      projectContextCoordinateKey,
    ),
    ...active.submittedDraft.contextCoordinates.map(
      projectContextCoordinateKey,
    ),
  ]);
  const documentIds = new Set<string>();
  for (const ids of active.overlay.relationDocumentIdsByEdge.values()) {
    for (const id of ids) documentIds.add(id);
  }
  for (const ids of active.overlay.rootRelationDocumentIdsByEdge.values()) {
    for (const id of ids) documentIds.add(id);
  }
  const detailByKey = new Map(
    substrate.coordinateDetails.map((detail) => [detail.coordinateKey, detail]),
  );
  const optionByKey = new Map(
    coordinateOptions.map((option) => [option.coordinateKey, option]),
  );
  const documentById = new Map(
    substrate.documentDetails.map((document) => [
      document.documentId,
      document,
    ]),
  );
  return JSON.stringify({
    coordinates: [...coordinateKeys].sort().map((key) => {
      const detail = detailByKey.get(key);
      const option = optionByKey.get(key);
      return {
        key,
        state: detail?.state ?? option?.state ?? "missing",
        title: detail?.title ?? option?.title ?? null,
        summary: detail?.summary ?? option?.description ?? null,
        status: detail?.status ?? option?.status ?? null,
        objectRevision: detail?.objectRevision ?? null,
        documentRevision: detail?.documentRevision ?? null,
        meeting: detail?.meeting ?? null,
        updatedAt: detail?.updatedAt ?? null,
      };
    }),
    documents: [...documentIds].sort().map((id) => {
      const document = documentById.get(id);
      return {
        id,
        state: document?.state ?? "missing",
        title: document?.title ?? null,
        summary: document?.summary ?? null,
        revision: document?.documentRevision ?? null,
        updatedAt: document?.updatedAt ?? null,
      };
    }),
  });
}
