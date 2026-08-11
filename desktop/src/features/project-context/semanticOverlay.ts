import {
  projectContextCoordinateNodeId,
  projectContextHubNodeId,
} from "@/features/project-context/graph";
import type { ProjectContextQueryResult } from "@/shared/api/tauriProjectContext";
import type {
  SemanticProjectContextPath,
  SemanticProjectContextPathHop,
  SemanticProjectContextQueryResult,
  SemanticProjectContextRoot,
} from "@/shared/api/tauriProjectContextSemantic";

export type ProjectContextSemanticRootEntrypoint =
  SemanticProjectContextRoot["contextDocumentEntrypoints"][number];
export type ProjectContextSemanticRoot = SemanticProjectContextRoot;
export type ProjectContextSemanticHop = SemanticProjectContextPathHop;
export type ProjectContextSemanticPath = Pick<
  SemanticProjectContextPath,
  "pathId" | "rootId" | "hops"
>;

/** Closed, verified display fields consumed by the pure graph overlay mapper. */
export type ProjectContextSemanticResultForOverlay = Pick<
  SemanticProjectContextQueryResult,
  | "communityKey"
  | "requestId"
  | "projectId"
  | "relayPubkey"
  | "projectContextRevision"
  | "roots"
> & { paths: ProjectContextSemanticPath[] };

export type ProjectContextSemanticOverlay = {
  communityKey: string;
  requestId: string;
  projectId: string;
  relayPubkey: string;
  projectContextRevision: number;
  substrateIdentity: string;
  pathCount: number;
  rootCount: number;
  edgeKeys: ReadonlySet<string>;
  rootEdgeKeys: ReadonlySet<string>;
  memberCoordinateKeys: ReadonlySet<string>;
  routeCoordinateKeys: ReadonlySet<string>;
  rootCoordinateKeys: ReadonlySet<string>;
  terminalCoordinateKeys: ReadonlySet<string>;
  relationDocumentIdsByEdge: ReadonlyMap<string, ReadonlySet<string>>;
  rootRelationDocumentIdsByEdge: ReadonlyMap<string, ReadonlySet<string>>;
  boundsTargetIds: readonly string[];
};

export type ProjectContextSemanticFreshness = "snapshot" | "stale";

export type ProjectContextSemanticOverlayMismatch =
  | "not_all_context"
  | "identity_mismatch"
  | "revision_mismatch"
  | "missing_root"
  | "missing_root_edge"
  | "missing_root_document"
  | "missing_path_root"
  | "missing_edge"
  | "coordinate_set_mismatch"
  | "binding_set_mismatch"
  | "selected_document_mismatch"
  | "route_coordinate_mismatch";

export type ProjectContextSemanticOverlayBuildResult =
  | { ok: true; overlay: ProjectContextSemanticOverlay }
  | { ok: false; reason: ProjectContextSemanticOverlayMismatch };

function canonicalValues(values: readonly string[]): string[] | null {
  const unique = new Set(values);
  if (unique.size !== values.length) return null;
  return [...unique].sort();
}

function sameCanonicalSet(
  left: readonly string[],
  right: readonly string[],
): boolean {
  const canonicalLeft = canonicalValues(left);
  const canonicalRight = canonicalValues(right);
  if (!canonicalLeft || !canonicalRight) return false;
  return (
    canonicalLeft.length === canonicalRight.length &&
    canonicalLeft.every((value, index) => value === canonicalRight[index])
  );
}

function addMapValue(
  target: Map<string, Set<string>>,
  key: string,
  value: string,
) {
  const values = target.get(key);
  if (values) {
    values.add(value);
  } else {
    target.set(key, new Set([value]));
  }
}

/**
 * Stable structural identity for the complete substrate used by the
 * render-time safety gate. It deliberately excludes titles and summaries.
 */
export function projectContextSemanticSubstrateIdentity(
  substrate: ProjectContextQueryResult,
): string {
  return JSON.stringify({
    communityKey: substrate.communityKey,
    contextRevision: substrate.context.contextRevision,
    edges: [...substrate.edges]
      .map((edge) => ({
        edgeKey: edge.edgeKey,
        coordinateKeys: [...edge.coordinateKeys].sort(),
        contextDocumentIds: [...edge.contextDocumentIds].sort(),
      }))
      .sort((left, right) => left.edgeKey.localeCompare(right.edgeKey, "en")),
    projectId: substrate.projectId,
    relayPubkey: substrate.relayPubkey.toLowerCase(),
  });
}

/** Synchronous fail-closed guard used immediately before graph presentation. */
export function semanticOverlayMatchesSubstrate(
  overlay: ProjectContextSemanticOverlay,
  substrate: ProjectContextQueryResult,
): boolean {
  return (
    substrate.query.type === "contains_all" &&
    substrate.query.coordinates.length === 0 &&
    overlay.communityKey === substrate.communityKey &&
    overlay.projectId === substrate.projectId &&
    overlay.relayPubkey.toLowerCase() === substrate.relayPubkey.toLowerCase() &&
    overlay.projectContextRevision === substrate.context.contextRevision &&
    overlay.substrateIdentity ===
      projectContextSemanticSubstrateIdentity(substrate)
  );
}

/**
 * Atomically validate one verified semantic result against one verified All
 * Context substrate and derive the presentation-only union overlay.
 */
export function buildProjectContextSemanticOverlay(
  result: ProjectContextSemanticResultForOverlay,
  substrate: ProjectContextQueryResult,
): ProjectContextSemanticOverlayBuildResult {
  if (
    substrate.query.type !== "contains_all" ||
    substrate.query.coordinates.length !== 0
  ) {
    return { ok: false, reason: "not_all_context" };
  }
  if (
    result.communityKey !== substrate.communityKey ||
    result.projectId !== substrate.projectId ||
    result.relayPubkey.toLowerCase() !== substrate.relayPubkey.toLowerCase()
  ) {
    return { ok: false, reason: "identity_mismatch" };
  }
  if (result.projectContextRevision !== substrate.context.contextRevision) {
    return { ok: false, reason: "revision_mismatch" };
  }

  const edgesByKey = new Map(
    substrate.edges.map((edge) => [edge.edgeKey, edge]),
  );
  const structuralCoordinateKeys = new Set(
    substrate.edges.flatMap((edge) => edge.coordinateKeys),
  );
  const returnedRootIds = new Set(result.roots.map((root) => root.rootId));
  const edgeKeys = new Set<string>();
  const rootEdgeKeys = new Set<string>();
  const memberCoordinateKeys = new Set<string>();
  const routeCoordinateKeys = new Set<string>();
  const rootCoordinateKeys = new Set<string>();
  const terminalCoordinateKeys = new Set<string>();
  const relationDocumentIdsByEdge = new Map<string, Set<string>>();
  const rootRelationDocumentIdsByEdge = new Map<string, Set<string>>();

  for (const root of result.roots) {
    if (
      root.coordinateEntrypoints.length === 0 &&
      root.contextDocumentEntrypoints.length === 0
    ) {
      return { ok: false, reason: "missing_root" };
    }
    for (const coordinateKey of root.coordinateEntrypoints) {
      if (!structuralCoordinateKeys.has(coordinateKey)) {
        return { ok: false, reason: "missing_root" };
      }
      rootCoordinateKeys.add(coordinateKey);
    }
    for (const entrypoint of root.contextDocumentEntrypoints) {
      const edge = edgesByKey.get(entrypoint.edgeKey);
      if (!edge) return { ok: false, reason: "missing_root_edge" };
      if (!edge.contextDocumentIds.includes(entrypoint.documentId)) {
        return { ok: false, reason: "missing_root_document" };
      }
      rootEdgeKeys.add(entrypoint.edgeKey);
      addMapValue(
        rootRelationDocumentIdsByEdge,
        entrypoint.edgeKey,
        entrypoint.documentId,
      );
    }
  }

  for (const path of result.paths) {
    if (!returnedRootIds.has(path.rootId)) {
      return { ok: false, reason: "missing_path_root" };
    }
    for (const [hopIndex, hop] of path.hops.entries()) {
      const edge = edgesByKey.get(hop.edgeKey);
      if (!edge) return { ok: false, reason: "missing_edge" };
      if (!sameCanonicalSet(hop.completeCoordinateKeys, edge.coordinateKeys)) {
        return { ok: false, reason: "coordinate_set_mismatch" };
      }
      if (
        !sameCanonicalSet(
          hop.currentContextDocumentIds,
          edge.contextDocumentIds,
        )
      ) {
        return { ok: false, reason: "binding_set_mismatch" };
      }
      if (!edge.contextDocumentIds.includes(hop.selectedContextDocumentId)) {
        return { ok: false, reason: "selected_document_mismatch" };
      }
      if (
        (hop.enteredFromCoordinateKey !== undefined &&
          !edge.coordinateKeys.includes(hop.enteredFromCoordinateKey)) ||
        !edge.coordinateKeys.includes(hop.continuedToCoordinateKey)
      ) {
        return { ok: false, reason: "route_coordinate_mismatch" };
      }

      edgeKeys.add(hop.edgeKey);
      for (const coordinateKey of edge.coordinateKeys) {
        memberCoordinateKeys.add(coordinateKey);
      }
      if (hop.enteredFromCoordinateKey !== undefined) {
        routeCoordinateKeys.add(hop.enteredFromCoordinateKey);
      }
      routeCoordinateKeys.add(hop.continuedToCoordinateKey);
      if (hopIndex === path.hops.length - 1) {
        terminalCoordinateKeys.add(hop.continuedToCoordinateKey);
      }
      addMapValue(
        relationDocumentIdsByEdge,
        hop.edgeKey,
        hop.selectedContextDocumentId,
      );
    }
  }

  const boundsTargetIds = [
    ...new Set([
      ...[...memberCoordinateKeys].map(projectContextCoordinateNodeId),
      ...[...rootCoordinateKeys].map(projectContextCoordinateNodeId),
      ...[...edgeKeys].map(projectContextHubNodeId),
      ...[...rootEdgeKeys].map(projectContextHubNodeId),
    ]),
  ].sort();

  return {
    ok: true,
    overlay: {
      communityKey: result.communityKey,
      requestId: result.requestId,
      projectId: result.projectId,
      relayPubkey: result.relayPubkey.toLowerCase(),
      projectContextRevision: result.projectContextRevision,
      substrateIdentity: projectContextSemanticSubstrateIdentity(substrate),
      pathCount: result.paths.length,
      rootCount: result.roots.length,
      edgeKeys,
      rootEdgeKeys,
      memberCoordinateKeys,
      routeCoordinateKeys,
      rootCoordinateKeys,
      terminalCoordinateKeys,
      relationDocumentIdsByEdge,
      rootRelationDocumentIdsByEdge,
      boundsTargetIds,
    },
  };
}
