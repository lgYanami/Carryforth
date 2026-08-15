import type { ProjectViewExplorerSelection } from "@/features/project-view/explorerModel";

export type ProjectViewRouteSearch = {
  object?: string;
  document?: string;
  revision?: number;
  via?: string;
};

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function positiveSafeInteger(value: unknown): number | undefined {
  const parsed =
    typeof value === "number"
      ? value
      : typeof value === "string"
        ? Number(value)
        : undefined;
  return parsed !== undefined && Number.isSafeInteger(parsed) && parsed > 0
    ? parsed
    : undefined;
}

/** Validate and canonicalize the mutually exclusive Project View route identity. */
export function validateProjectViewRouteSearch(
  search: Record<string, unknown>,
): ProjectViewRouteSearch {
  const via = nonEmptyString(search.via);
  const object = nonEmptyString(search.object);
  if (object) {
    return { object, via };
  }
  const document = nonEmptyString(search.document);
  if (document) {
    return {
      document,
      revision: positiveSafeInteger(search.revision),
      via,
    };
  }
  return {};
}

/** Convert canonical URL state into an untrusted Explorer selection request. */
export function projectViewSelectionFromRoute(
  search: ProjectViewRouteSearch,
): ProjectViewExplorerSelection | undefined {
  if (search.document) {
    return {
      kind: "document",
      documentId: search.document,
      revision: search.revision,
      via: search.via,
    };
  }
  return search.object
    ? { kind: "object", objectId: search.object, via: search.via }
    : undefined;
}

/** Build a complete route search value; callers must never merge prior identity fields. */
export function projectViewRouteForSelection(
  selection?: ProjectViewExplorerSelection,
): ProjectViewRouteSearch {
  if (!selection) return {};
  return selection.kind === "object"
    ? { object: selection.objectId, via: selection.via }
    : {
        document: selection.documentId,
        revision: selection.revision,
        via: selection.via,
      };
}
