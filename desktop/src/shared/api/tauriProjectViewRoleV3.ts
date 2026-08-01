import { ProjectViewIntegrityError } from "@/shared/api/tauriProjectViewIntegrity";

export type ProjectRoleBriefBaseContextV3 =
  | { availability: "not_advertised_empty" }
  | {
      availability: "unavailable_preserved";
      resourceCount: number;
      documentCount: number;
    };

function record(value: unknown, label: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new ProjectViewIntegrityError(`${label} is not an object`);
  }
  return value as Record<string, unknown>;
}

function hasExactKeys(value: Record<string, unknown>, keys: string[]) {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return (
    actual.length === expected.length &&
    actual.every((key, index) => key === expected[index])
  );
}

function nonnegativeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && typeof value === "number" && value >= 0;
}

/** Validate the strict, non-hydrated RoleBriefV3 surface returned in stage 5. */
export function validateBaseRoleBriefV3(
  raw: Record<string, unknown>,
  projectRevision: number,
  projectionGeneration: number,
): ProjectRoleBriefBaseContextV3 {
  if (
    raw.project_view_schema_version !== 3 ||
    raw.project_revision !== projectRevision ||
    raw.projection_generation !== projectionGeneration
  ) {
    throw new ProjectViewIntegrityError(
      "RoleBriefV3 does not match the verified Project snapshot",
    );
  }

  const sourceRevisions = record(
    raw.source_revisions,
    "RoleBriefV3 source revisions",
  );
  const documentMetadata = record(
    sourceRevisions.document_metadata,
    "RoleBriefV3 Document metadata boundary",
  );
  if (
    !hasExactKeys(documentMetadata, ["state"]) ||
    documentMetadata.state !== "not_required"
  ) {
    throw new ProjectViewIntegrityError(
      "base RoleBriefV3 must use document_metadata:not_required",
    );
  }

  const context = record(raw.context, "RoleBriefV3 Context");
  const resources = context.resources;
  const liveDocuments = context.live_documents;
  const pinnedDocuments = context.pinned_documents;
  const truncation = record(
    context.truncation,
    "RoleBriefV3 Context truncation",
  );
  if (
    !hasExactKeys(context, [
      "availability",
      "resources",
      "live_documents",
      "pinned_documents",
      "truncation",
    ]) ||
    !Array.isArray(resources) ||
    resources.length !== 0 ||
    !Array.isArray(liveDocuments) ||
    liveDocuments.length !== 0 ||
    !Array.isArray(pinnedDocuments) ||
    pinnedDocuments.length !== 0 ||
    !hasExactKeys(truncation, [
      "truncated",
      "omitted_resources",
      "omitted_live_documents",
      "omitted_pinned_documents",
    ]) ||
    truncation.truncated !== false ||
    truncation.omitted_resources !== 0 ||
    truncation.omitted_live_documents !== 0 ||
    truncation.omitted_pinned_documents !== 0
  ) {
    throw new ProjectViewIntegrityError(
      "base RoleBriefV3 must not hydrate or truncate Context",
    );
  }

  const availability = record(
    context.availability,
    "RoleBriefV3 Context availability",
  );
  if (
    hasExactKeys(availability, ["state"]) &&
    availability.state === "not_advertised_empty"
  ) {
    return { availability: "not_advertised_empty" };
  }
  if (
    hasExactKeys(availability, ["state", "resource_count", "document_count"]) &&
    availability.state === "unavailable_preserved" &&
    nonnegativeInteger(availability.resource_count) &&
    nonnegativeInteger(availability.document_count) &&
    availability.resource_count + availability.document_count > 0
  ) {
    return {
      availability: "unavailable_preserved",
      resourceCount: availability.resource_count,
      documentCount: availability.document_count,
    };
  }
  throw new ProjectViewIntegrityError(
    "base RoleBriefV3 has an invalid Context availability state",
  );
}
