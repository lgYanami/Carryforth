import {
  formatProjectViewTerm,
  projectViewObjectStatus,
  projectViewObjectTitle,
  projectViewObjectTypeLabel,
} from "@/features/project-view/model";
import type { ProjectDocumentListItem } from "@/shared/api/tauriProjectDocument";
import type { UserProfileSummary } from "@/shared/api/types";
import type { MeetingListItem } from "@/shared/api/tauriMeetings";
import {
  canonicalizeProjectContextCoordinates,
  canonicalizeProjectContextQuery,
  projectContextCoordinateKey,
  type ProjectContextCoordinate,
  type ProjectContextCoordinateDetail,
  type ProjectContextDetailState,
  type ProjectContextQuery,
} from "@/shared/api/tauriProjectContext";
import type { ProjectViewObject } from "@/shared/api/tauriProjectView";
import { truncatePubkey } from "@/shared/lib/pubkey";
import {
  projectContextModeForQuery,
  type ProjectContextQueryMode,
} from "@/features/project-context/routeState";

export type ProjectContextQueryDraft = {
  mode: ProjectContextQueryMode;
  coordinates: ProjectContextCoordinate[];
};

export type ProjectContextDraftCoordinateRejection =
  | "mode_all"
  | "duplicate"
  | "incident_full";

export type ProjectContextDraftCoordinateTransition =
  | { status: "changed"; draft: ProjectContextQueryDraft }
  | {
      status: "unchanged";
      draft: ProjectContextQueryDraft;
      reason: ProjectContextDraftCoordinateRejection;
    };

export type ProjectContextCoordinateOption = {
  coordinate: ProjectContextCoordinate;
  coordinateKey: string;
  group: "project_view" | "documents" | "meetings";
  state: ProjectContextDetailState;
  title: string;
  typeLabel: string;
  description?: string;
  status?: string;
  searchTerms?: string;
};

function coordinatesForQuery(
  query: ProjectContextQuery,
): ProjectContextCoordinate[] {
  const canonical = canonicalizeProjectContextQuery(query);
  return canonical.type === "incident"
    ? [canonical.coordinate]
    : canonical.coordinates;
}

/** Create a mutable Query Bar draft from the stable applied query. */
export function projectContextDraftFromQuery(
  query: ProjectContextQuery,
): ProjectContextQueryDraft {
  return {
    mode: projectContextModeForQuery(query),
    coordinates: coordinatesForQuery(query),
  };
}

/** Apply one mode change without manufacturing a domain query. */
export function changeProjectContextDraftMode(
  draft: ProjectContextQueryDraft,
  mode: ProjectContextQueryMode,
): ProjectContextQueryDraft {
  return {
    mode,
    coordinates:
      mode === "all"
        ? []
        : mode === "incident"
          ? draft.coordinates.slice(0, 1)
          : draft.coordinates,
  };
}

/**
 * Try to add one Coordinate without throwing for recoverable UI input.
 *
 * Strict query validation remains at the closed query/native boundary. Query
 * Bar interactions instead stay total so stale or repeated input cannot tear
 * down the React route.
 */
export function tryAddProjectContextDraftCoordinate(
  draft: ProjectContextQueryDraft,
  coordinate: ProjectContextCoordinate,
): ProjectContextDraftCoordinateTransition {
  if (draft.mode === "all") {
    return { status: "unchanged", draft, reason: "mode_all" };
  }
  const key = projectContextCoordinateKey(coordinate);
  if (
    draft.coordinates.some(
      (candidate) => projectContextCoordinateKey(candidate) === key,
    )
  ) {
    return { status: "unchanged", draft, reason: "duplicate" };
  }
  if (draft.mode === "incident" && draft.coordinates.length === 1) {
    return { status: "unchanged", draft, reason: "incident_full" };
  }
  return {
    status: "changed",
    draft: {
      ...draft,
      coordinates: canonicalizeProjectContextCoordinates([
        ...draft.coordinates,
        coordinate,
      ]),
    },
  };
}

/** Add one Coordinate as an idempotent Query Bar state transition. */
export function addProjectContextDraftCoordinate(
  draft: ProjectContextQueryDraft,
  coordinate: ProjectContextCoordinate,
): ProjectContextQueryDraft {
  return tryAddProjectContextDraftCoordinate(draft, coordinate).draft;
}

/** Remove one Coordinate without changing the currently applied query. */
export function removeProjectContextDraftCoordinate(
  draft: ProjectContextQueryDraft,
  coordinateKey: string,
): ProjectContextQueryDraft {
  return {
    ...draft,
    coordinates: draft.coordinates.filter(
      (coordinate) => projectContextCoordinateKey(coordinate) !== coordinateKey,
    ),
  };
}

/** Return the current mode constraint failure, if any. */
export function projectContextDraftValidationMessage(
  draft: ProjectContextQueryDraft,
): string | undefined {
  switch (draft.mode) {
    case "all":
      return draft.coordinates.length === 0
        ? undefined
        : "All Context does not accept Coordinates.";
    case "exact":
      return draft.coordinates.length >= 2
        ? undefined
        : "Exact requires at least two distinct Coordinates.";
    case "incident":
      return draft.coordinates.length === 1
        ? undefined
        : "Incident requires exactly one Coordinate.";
    case "contains_all":
      return draft.coordinates.length >= 1
        ? undefined
        : "Contains all requires at least one Coordinate; use All Context for the empty set.";
  }
}

/** Convert a valid draft into the closed query union submitted to native. */
export function projectContextQueryFromDraft(
  draft: ProjectContextQueryDraft,
): ProjectContextQuery {
  const validation = projectContextDraftValidationMessage(draft);
  if (validation) throw new Error(validation);
  if (draft.mode === "all") {
    return { type: "contains_all", coordinates: [] };
  }
  if (draft.mode === "incident") {
    return canonicalizeProjectContextQuery({
      type: "incident",
      coordinate: draft.coordinates[0],
    });
  }
  return canonicalizeProjectContextQuery({
    type: draft.mode,
    coordinates: draft.coordinates,
  });
}

function shortId(value: string): string {
  return value.length > 12 ? `${value.slice(0, 8)}…${value.slice(-4)}` : value;
}

function meetingLifecycleLabel(
  lifecycle: MeetingListItem["lifecycle"],
): string {
  switch (lifecycle) {
    case "finalizing_actions":
      return "Finalizing actions";
    case "closed":
      return "Closed";
    case "aborted":
      return "Aborted";
    case "initializing":
      return "Initializing";
    default:
      return "In progress";
  }
}

function optionFromVisibleDetail(
  detail: ProjectContextCoordinateDetail,
): ProjectContextCoordinateOption {
  const coordinate = detail.coordinate;
  const stableId =
    coordinate.type === "document"
      ? coordinate.documentId
      : coordinate.type === "meeting"
        ? coordinate.meetingId
        : coordinate.objectId;
  const typeLabel =
    coordinate.type === "document"
      ? "Document"
      : coordinate.type === "meeting"
        ? "Meeting"
        : projectViewObjectTypeLabel(coordinate.objectType);
  return {
    coordinate,
    coordinateKey: projectContextCoordinateKey(coordinate),
    group:
      coordinate.type === "document"
        ? "documents"
        : coordinate.type === "meeting"
          ? "meetings"
          : "project_view",
    state: detail.state,
    title: detail.title?.trim() || `${typeLabel} ${shortId(stableId)}`,
    typeLabel,
    description: detail.meeting?.discussionGoal ?? detail.unavailableReason,
    status: detail.meeting
      ? meetingLifecycleLabel(detail.meeting.lifecycle)
      : undefined,
    searchTerms: detail.meeting
      ? [
          detail.meeting.hostPubkey,
          ...detail.meeting.participantPreview.map(
            (participant) => participant.pubkey,
          ),
        ].join(" ")
      : undefined,
  };
}

function compareOptions(
  left: ProjectContextCoordinateOption,
  right: ProjectContextCoordinateOption,
) {
  const groupRank = { project_view: 0, documents: 1, meetings: 2 } as const;
  const group = groupRank[left.group] - groupRank[right.group];
  return (
    group ||
    left.typeLabel.localeCompare(right.typeLabel, "en") ||
    left.title.localeCompare(right.title, "en") ||
    left.coordinateKey.localeCompare(right.coordinateKey, "en")
  );
}

/**
 * Build one current-project picker catalog. Active source catalogs enrich but
 * never erase visible tombstoned or unavailable graph Coordinates.
 */
export function buildProjectContextCoordinateOptions(input: {
  projectViewObjects?: Iterable<ProjectViewObject>;
  documents?: ProjectDocumentListItem[];
  meetings?: MeetingListItem[];
  profiles?: Record<string, UserProfileSummary>;
  visibleDetails?: ProjectContextCoordinateDetail[];
}): ProjectContextCoordinateOption[] {
  const options = new Map<string, ProjectContextCoordinateOption>();
  for (const detail of input.visibleDetails ?? []) {
    const option = optionFromVisibleDetail(detail);
    options.set(option.coordinateKey, option);
  }
  for (const object of input.projectViewObjects ?? []) {
    const coordinate = {
      type: "project_view_object" as const,
      objectType: object.objectType,
      objectId: object.id,
    };
    const status = projectViewObjectStatus(object);
    const option: ProjectContextCoordinateOption = {
      coordinate,
      coordinateKey: projectContextCoordinateKey(coordinate),
      group: "project_view",
      state: "active",
      title: projectViewObjectTitle(object),
      typeLabel: projectViewObjectTypeLabel(object.objectType),
      status: status ? formatProjectViewTerm(status) : undefined,
    };
    options.set(option.coordinateKey, option);
  }
  for (const document of input.documents ?? []) {
    const coordinate = {
      type: "document" as const,
      documentId: document.documentId,
    };
    const option: ProjectContextCoordinateOption = {
      coordinate,
      coordinateKey: projectContextCoordinateKey(coordinate),
      group: "documents",
      state: "active",
      title: document.title,
      typeLabel: "Document",
      description: document.summary,
    };
    options.set(option.coordinateKey, option);
  }
  for (const meeting of input.meetings ?? []) {
    if (
      meeting.compatibility !== "ready" ||
      (meeting.lifecycle !== "finalizing_actions" &&
        meeting.lifecycle !== "closed" &&
        meeting.lifecycle !== "aborted")
    ) {
      continue;
    }
    const coordinate = {
      type: "meeting" as const,
      meetingId: meeting.meetingId,
    };
    const profileName = (pubkey: string) =>
      input.profiles?.[pubkey.toLowerCase()]?.displayName?.trim() ||
      truncatePubkey(pubkey);
    const participantNames = meeting.participantPreview.map((participant) =>
      profileName(participant.pubkey),
    );
    const remaining = Math.max(
      0,
      (meeting.participantCount ?? participantNames.length) -
        participantNames.length,
    );
    const participantSummary = [
      ...participantNames,
      remaining > 0 ? `+${remaining}` : undefined,
    ]
      .filter(Boolean)
      .join(", ");
    const option: ProjectContextCoordinateOption = {
      coordinate,
      coordinateKey: projectContextCoordinateKey(coordinate),
      group: "meetings",
      state: meeting.lifecycle === "finalizing_actions" ? "active" : "terminal",
      title: meeting.title,
      typeLabel: "Meeting",
      description:
        meeting.description ||
        (participantSummary
          ? `Participants: ${participantSummary}`
          : undefined),
      status: `${meetingLifecycleLabel(meeting.lifecycle)}${
        meeting.endedAt
          ? ` · ${new Date(meeting.endedAt * 1_000).toLocaleString()}`
          : ""
      }`,
      searchTerms: [
        meeting.hostPubkey,
        meeting.hostPubkey ? profileName(meeting.hostPubkey) : undefined,
        participantSummary,
        ...meeting.participantPreview.flatMap((participant) => [
          participant.pubkey,
          profileName(participant.pubkey),
        ]),
      ]
        .filter(Boolean)
        .join(" "),
    };
    options.set(option.coordinateKey, option);
  }
  return [...options.values()].sort(compareOptions);
}
