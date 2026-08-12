import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import type { SubmittedSemanticQueryDraft } from "@/features/project-context/semanticQueryModel";
import type { ProjectContextPickerSourceState } from "@/features/project-context/ui/ProjectContextQueryBar";
import type {
  ProjectContextQuery,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";

export type ValidProjectContextScreenProps = {
  appliedQuery: ProjectContextQuery;
  onApplyQuery: (query: ProjectContextQuery) => void;
  onOpenDocument: (documentId: string) => void;
  onOpenMeeting: (meetingId: string) => void;
  onOpenProjectView: (objectId: string) => void;
  onSelectionChange: (
    selection: ProjectContextRouteSelection | null,
    options?: { replace?: boolean },
  ) => void;
  selection: ProjectContextRouteSelection | null;
};

export type InvalidProjectContextScreenProps = {
  onResetRoute: () => void;
  routeError: string;
};

/** Map independent canonical picker reads into one closed presentation state. */
export function projectContextPickerSourceState(input: {
  error: unknown;
  loading: boolean;
  ready: boolean;
}): ProjectContextPickerSourceState {
  if (input.ready) return "ready";
  if (input.error) return "unavailable";
  if (input.loading) return "loading";
  return "unavailable";
}

/** Whether the current verified substrate can still render a route selection. */
export function projectContextSelectionRemainsVisible(
  result: ProjectContextQueryResult,
  selection: ProjectContextRouteSelection,
) {
  return selection.kind === "edge"
    ? result.edges.some((edge) => edge.edgeKey === selection.key)
    : result.coordinateDetails.some(
        (detail) => detail.coordinateKey === selection.key,
      ) ||
        result.edges.some((edge) =>
          edge.coordinateKeys.includes(selection.key),
        );
}

/** Closed presentation status for the collapsed Semantic Rail indicator. */
export function projectContextSemanticToolStatus({
  active,
  inFlight,
  stale,
}: {
  active: boolean;
  inFlight: boolean;
  stale: boolean;
}): "idle" | "running" | "active" | "stale" {
  if (inFlight) return "running";
  if (!active) return "idle";
  return stale ? "stale" : "active";
}

/** Closed keyed event for the workspace's sole live announcement owner. */
export function projectContextWorkspaceStateEvent({
  failureKind,
  pending,
  projectId,
  revision,
  syncMessage,
  syncState,
}: {
  failureKind?: string;
  pending: boolean;
  projectId: string;
  revision?: number;
  syncMessage?: string;
  syncState?: string;
}): { key: string; message: string } | undefined {
  if (failureKind) {
    return {
      key: `context:${projectId}:failure:${failureKind}`,
      message: "Project Context could not be displayed.",
    };
  }
  if (pending) {
    return {
      key: `context:${projectId}:loading`,
      message: "Reading and verifying the complete Project Context snapshot.",
    };
  }
  return syncMessage && revision !== undefined
    ? {
        key: `context:${revision}:${syncState ?? "unknown"}`,
        message: syncMessage,
      }
    : undefined;
}

/** Stable Meeting scope needed by semantic currentness observation. */
export function projectContextSemanticMeetingIds({
  coordinateDetails,
  enabled,
  submittedDrafts,
}: {
  coordinateDetails?: ProjectContextQueryResult["coordinateDetails"];
  enabled: boolean;
  submittedDrafts: readonly SubmittedSemanticQueryDraft[];
}) {
  if (!enabled) return [];
  const ids = new Set<string>();
  for (const detail of coordinateDetails ?? []) {
    if (detail.coordinate.type === "meeting") {
      ids.add(detail.coordinate.meetingId);
    }
  }
  for (const submitted of submittedDrafts) {
    for (const coordinate of [
      ...submitted.initialCoordinates,
      ...submitted.contextCoordinates,
    ]) {
      if (coordinate.type === "meeting") ids.add(coordinate.meetingId);
    }
  }
  return [...ids].sort();
}
