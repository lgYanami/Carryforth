import type { ProjectViewContextReference } from "@/shared/api/tauriProjectView";
import { ProjectViewIntegrityError } from "@/shared/api/tauriProjectViewIntegrity";

export function projectViewContextReferenceKey(
  reference: ProjectViewContextReference,
): string {
  return reference.referenceType === "resource"
    ? `resource:${reference.resourceId}`
    : `document:${reference.documentId}:${reference.mode}:${reference.documentRevision ?? 0}`;
}

function compareContextReferences(
  left: ProjectViewContextReference,
  right: ProjectViewContextReference,
): number {
  if (left.referenceType !== right.referenceType) {
    return left.referenceType === "resource" ? -1 : 1;
  }
  if (left.referenceType === "resource") {
    const rightResource = right as Extract<
      ProjectViewContextReference,
      { referenceType: "resource" }
    >;
    return left.resourceId < rightResource.resourceId
      ? -1
      : left.resourceId > rightResource.resourceId
        ? 1
        : 0;
  }
  const rightDocument = right as Extract<
    ProjectViewContextReference,
    { referenceType: "document" }
  >;
  if (left.documentId !== rightDocument.documentId) {
    return left.documentId < rightDocument.documentId ? -1 : 1;
  }
  if (left.mode !== rightDocument.mode) return left.mode === "live" ? -1 : 1;
  return (left.documentRevision ?? 0) - (rightDocument.documentRevision ?? 0);
}

/** Validate and order the complete Context set before it crosses Tauri. */
export function canonicalizeProjectViewContextReferences(
  references: ProjectViewContextReference[],
): ProjectViewContextReference[] {
  if (references.length > 64) {
    throw new ProjectViewIntegrityError(
      "a Project View object may have at most 64 Context References",
    );
  }
  const canonical = references.map((reference) => {
    if (
      reference.referenceType === "document" &&
      ((reference.mode === "live" &&
        reference.documentRevision !== undefined) ||
        (reference.mode === "pinned" &&
          (!Number.isSafeInteger(reference.documentRevision) ||
            (reference.documentRevision ?? 0) < 1)))
    ) {
      throw new ProjectViewIntegrityError(
        "a live Document reference omits revision and a pinned reference requires a positive revision",
      );
    }
    return { ...reference };
  });
  canonical.sort(compareContextReferences);
  for (let index = 1; index < canonical.length; index += 1) {
    if (
      projectViewContextReferenceKey(canonical[index - 1]) ===
      projectViewContextReferenceKey(canonical[index])
    ) {
      throw new ProjectViewIntegrityError(
        "the Context Reference set contains a duplicate coordinate",
      );
    }
  }
  return canonical;
}

/** Reject a signed v3 object whose Context set is not already canonical. */
export function requireCanonicalProjectViewContextReferences(
  references: ProjectViewContextReference[],
): ProjectViewContextReference[] {
  const canonical = canonicalizeProjectViewContextReferences(references);
  if (
    canonical.some(
      (reference, index) =>
        projectViewContextReferenceKey(reference) !==
        projectViewContextReferenceKey(references[index]),
    )
  ) {
    throw new ProjectViewIntegrityError(
      "the verified v3 Context Reference set is not in canonical order",
    );
  }
  return canonical;
}
