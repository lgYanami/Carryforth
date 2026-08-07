import {
  canonicalizeProjectContextQuery,
  projectContextCoordinateFromKey,
  projectContextCoordinateKey,
  type ProjectContextCoordinate,
  type ProjectContextQuery,
} from "@/shared/api/tauriProjectContext";

export type ProjectContextQueryMode =
  | "all"
  | "exact"
  | "incident"
  | "contains_all";

export type ProjectContextRouteSelection =
  | { kind: "coordinate"; key: string }
  | { kind: "edge"; key: string };

export type ProjectContextRouteSearch = {
  mode?: "exact" | "incident" | "contains_all";
  coordinates?: string;
  selected?: string;
  invalid?: string;
};

export type ProjectContextRouteState = {
  query: ProjectContextQuery;
  selection: ProjectContextRouteSelection | null;
};

const EDGE_KEY_PATTERN = /^[0-9a-f]{64}$/i;

function errorMessage(error: unknown): string {
  return error instanceof Error
    ? error.message
    : "Project Context route search is invalid.";
}

function coordinatesFromSearch(value: unknown): ProjectContextCoordinate[] {
  if (typeof value !== "string") {
    throw new Error("Project Context coordinates must use one string value.");
  }
  if (value.trim() === "") return [];
  const tokens = value.split(",");
  if (tokens.some((token) => token.trim() === "")) {
    throw new Error("Project Context coordinates contain an empty token.");
  }
  return tokens.map((token) => projectContextCoordinateFromKey(token.trim()));
}

function selectionFromSearch(
  value: unknown,
): ProjectContextRouteSelection | null {
  if (value === undefined) return null;
  if (typeof value !== "string") {
    throw new Error("Project Context selection must use one string value.");
  }
  if (value.startsWith("coordinate:")) {
    const coordinate = projectContextCoordinateFromKey(
      value.slice("coordinate:".length),
    );
    return {
      kind: "coordinate",
      key: projectContextCoordinateKey(coordinate),
    };
  }
  if (value.startsWith("edge:")) {
    const key = value.slice("edge:".length).toLowerCase();
    if (EDGE_KEY_PATTERN.test(key)) return { kind: "edge", key };
  }
  throw new Error("Project Context selection is malformed.");
}

function queryFromSearch(search: Record<string, unknown>): ProjectContextQuery {
  const mode = search.mode;
  if (mode === undefined) {
    if (search.coordinates !== undefined) {
      throw new Error("Project Context coordinates require a query mode.");
    }
    return { type: "contains_all", coordinates: [] };
  }
  if (mode !== "exact" && mode !== "incident" && mode !== "contains_all") {
    throw new Error("Project Context query mode is not supported.");
  }
  const coordinates = coordinatesFromSearch(search.coordinates ?? "");
  if (mode === "incident") {
    if (coordinates.length !== 1) {
      throw new Error("Incident requires exactly one Coordinate.");
    }
    return canonicalizeProjectContextQuery({
      type: "incident",
      coordinate: coordinates[0],
    });
  }
  return canonicalizeProjectContextQuery({ type: mode, coordinates });
}

/** True only for the complete `contains-all({})` catalog query. */
export function isAllProjectContextQuery(query: ProjectContextQuery): boolean {
  const canonical = canonicalizeProjectContextQuery(query);
  return (
    canonical.type === "contains_all" && canonical.coordinates.length === 0
  );
}

/** Convert one domain query to its user-facing Query Bar mode. */
export function projectContextModeForQuery(
  query: ProjectContextQuery,
): ProjectContextQueryMode {
  const canonical = canonicalizeProjectContextQuery(query);
  return isAllProjectContextQuery(canonical) ? "all" : canonical.type;
}

/** Build the unique stable search representation for one query and selection. */
export function projectContextRouteSearchForState(
  query: ProjectContextQuery,
  selection: ProjectContextRouteSelection | null = null,
): ProjectContextRouteSearch {
  const canonical = canonicalizeProjectContextQuery(query);
  const selected = selection
    ? selection.kind === "coordinate"
      ? `coordinate:${projectContextCoordinateKey(
          projectContextCoordinateFromKey(selection.key),
        )}`
      : EDGE_KEY_PATTERN.test(selection.key)
        ? `edge:${selection.key.toLowerCase()}`
        : undefined
    : undefined;
  if (selection?.kind === "edge" && !selected) {
    throw new Error("Project Context Edge selection is malformed.");
  }
  if (isAllProjectContextQuery(canonical)) {
    return selected ? { selected } : {};
  }
  const coordinates =
    canonical.type === "incident"
      ? [canonical.coordinate]
      : canonical.coordinates;
  return {
    mode: canonical.type,
    coordinates: coordinates.map(projectContextCoordinateKey).join(","),
    ...(selected ? { selected } : {}),
  };
}

/** Parse one already-validated route search into typed domain state. */
export function projectContextRouteStateFromSearch(
  search: ProjectContextRouteSearch,
): ProjectContextRouteState {
  if (search.invalid) throw new Error(search.invalid);
  return {
    query: queryFromSearch(search),
    selection: selectionFromSearch(search.selected),
  };
}

/** Closed route validator: unknown fields are dropped; malformed state is explicit. */
export function validateProjectContextRouteSearch(
  search: Record<string, unknown>,
): ProjectContextRouteSearch {
  try {
    const state = {
      query: queryFromSearch(search),
      selection: selectionFromSearch(search.selected),
    };
    return projectContextRouteSearchForState(state.query, state.selection);
  } catch (error) {
    return { invalid: errorMessage(error) };
  }
}
