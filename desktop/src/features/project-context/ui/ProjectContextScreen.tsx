import {
  AlertTriangle,
  FileText,
  Network,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import * as React from "react";

import {
  useProjectDocumentMeta,
  useProjectDocuments,
} from "@/features/project-documents/hooks";
import { useChannelsQuery } from "@/features/channels/hooks";
import {
  useMeetingDirectory,
  useMeetingLiveSync,
} from "@/features/meeting/hooks";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import {
  buildProjectContextCoordinateOptions,
  type ProjectContextCoordinateOption,
} from "@/features/project-context/queryModel";
import { focusProjectContextGraphTarget } from "@/features/project-context/focus";
import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import { isAllProjectContextQuery } from "@/features/project-context/routeState";
import {
  projectContextErrorMessage,
  projectContextFailureKind,
  visibleContextDocumentCount,
} from "@/features/project-context/state";
import { ProjectContextGraph } from "@/features/project-context/ui/ProjectContextGraph";
import { ProjectContextInspector } from "@/features/project-context/ui/ProjectContextInspector";
import {
  type ProjectContextPickerSourceState,
  ProjectContextQueryBar,
} from "@/features/project-context/ui/ProjectContextQueryBar";
import {
  ProjectContextEmptyState,
  ProjectContextFailureState,
  ProjectContextLoadingState,
} from "@/features/project-context/ui/ProjectContextStates";
import {
  useProjectContextLiveSync,
  useProjectContextQuery,
} from "@/features/project-context/hooks";
import { useProjectViewQuery } from "@/features/project-view/hooks";
import { indexProjectViewObjects } from "@/features/project-view/model";
import type {
  ProjectContextQuery,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";
import {
  isRelayConnectionDegraded,
  useRelayConnection,
} from "@/shared/api/useRelayConnection";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

type ValidProjectContextScreenProps = {
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

type InvalidProjectContextScreenProps = {
  onResetRoute: () => void;
  routeError: string;
};

function ProjectContextHeader({
  onRefresh,
  refreshing,
  result,
  syncBadge,
  syncState,
}: {
  onRefresh?: () => void;
  refreshing?: boolean;
  result?: ProjectContextQueryResult;
  syncBadge?: string;
  syncState?: "live" | "refreshing" | "stale";
}) {
  return (
    <TopChromeInsetHeader flush>
      <header
        className="flex h-12 items-center gap-2 px-3 sm:gap-3 sm:px-5"
        data-tauri-drag-region
      >
        <Network className="h-4 w-4 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">Project Context</div>
          <div className="hidden text-2xs text-muted-foreground sm:block">
            Verified, read-only relationships across project coordinates
          </div>
        </div>
        {result ? (
          <Badge variant="success">
            <ShieldCheck className="mr-1 h-3 w-3" />
            Verified
          </Badge>
        ) : null}
        {result && !result.context.capabilityEnabled ? (
          <Badge variant="warning">Capability off · read-only</Badge>
        ) : null}
        {result ? (
          <Badge className="hidden sm:inline-flex" variant="outline">
            Revision {result.context.contextRevision}
          </Badge>
        ) : null}
        {syncState ? (
          <Badge
            data-testid="project-context-sync-status"
            variant={
              syncState === "stale"
                ? "warning"
                : syncState === "live"
                  ? "success"
                  : "secondary"
            }
          >
            {syncState === "refreshing" ? (
              <RefreshCw className="mr-1 h-3 w-3 animate-spin" />
            ) : null}
            {syncBadge ??
              (syncState === "stale"
                ? "Stale"
                : syncState === "live"
                  ? "Live"
                  : "Syncing")}
          </Badge>
        ) : null}
        {onRefresh ? (
          <Button
            aria-label="Refresh Project Context"
            data-testid="project-context-refresh"
            disabled={refreshing}
            onClick={onRefresh}
            size="icon"
            type="button"
            variant="ghost"
          >
            <RefreshCw
              className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`}
            />
          </Button>
        ) : null}
      </header>
    </TopChromeInsetHeader>
  );
}

function ProjectContextGraphSlot({
  focusSelectionRequest,
  onSelectionChange,
  result,
  selection,
}: {
  focusSelectionRequest: number;
  onSelectionChange: ValidProjectContextScreenProps["onSelectionChange"];
  result: ProjectContextQueryResult;
  selection: ProjectContextRouteSelection | null;
}) {
  const documentCount = visibleContextDocumentCount(result);
  return (
    <main className="min-h-0 min-w-0 flex-1 overflow-auto p-4 sm:p-6">
      <div className="mx-auto flex h-full min-h-80 max-w-6xl flex-col gap-4">
        <section
          className="grid gap-3 sm:grid-cols-3"
          data-context-document-count={documentCount}
          data-coordinate-count={result.coordinateDetails.length}
          data-edge-count={result.edges.length}
          data-testid="project-context-result-counts"
        >
          <div className="rounded-xl border border-border/70 bg-card/60 p-3">
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Matching Edges
            </div>
            <div className="mt-1 text-xl font-semibold">
              {result.edges.length}
            </div>
          </div>
          <div className="rounded-xl border border-border/70 bg-card/60 p-3">
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Visible Coordinates
            </div>
            <div className="mt-1 text-xl font-semibold">
              {result.coordinateDetails.length}
            </div>
          </div>
          <div className="rounded-xl border border-border/70 bg-card/60 p-3">
            <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
              Context Documents
            </div>
            <div className="mt-1 text-xl font-semibold">{documentCount}</div>
          </div>
        </section>
        <div
          className="flex min-h-96 flex-1"
          data-testid="project-context-graph-slot"
        >
          <ProjectContextGraph
            focusSelectionRequest={focusSelectionRequest}
            onSelectionChange={onSelectionChange}
            result={result}
            selection={selection}
          />
        </div>
      </div>
    </main>
  );
}

function pickerSourceState(input: {
  error: unknown;
  loading: boolean;
  ready: boolean;
}): ProjectContextPickerSourceState {
  if (input.ready) return "ready";
  if (input.error) return "unavailable";
  if (input.loading) return "loading";
  return "unavailable";
}

function ValidProjectContextScreen({
  appliedQuery,
  onApplyQuery,
  onOpenDocument,
  onOpenMeeting,
  onOpenProjectView,
  onSelectionChange,
  selection,
}: ValidProjectContextScreenProps) {
  const [focusSelectionRequest, setFocusSelectionRequest] = React.useState(0);
  const contextQuery = useProjectContextQuery(appliedQuery);
  const liveStatus = useProjectContextLiveSync(contextQuery.data);
  const relayConnection = useRelayConnection();
  const projectViewQuery = useProjectViewQuery();
  const documentMetaQuery = useProjectDocumentMeta();
  const documentsQuery = useProjectDocuments(documentMetaQuery.data);
  const channelsQuery = useChannelsQuery();
  const meetingIds = React.useMemo(
    () =>
      (channelsQuery.data ?? [])
        .filter((channel) => channel.roomKind === "meeting")
        .map((channel) => channel.id)
        .sort(),
    [channelsQuery.data],
  );
  const meetingDirectoryQuery = useMeetingDirectory(meetingIds);
  useMeetingLiveSync(meetingIds, meetingDirectoryQuery.data);
  const meetingProfilePubkeys = React.useMemo(
    () =>
      [
        ...new Set(
          (meetingDirectoryQuery.data ?? []).flatMap((meeting) => [
            ...(meeting.hostPubkey ? [meeting.hostPubkey] : []),
            ...meeting.participantPreview.map(
              (participant) => participant.pubkey,
            ),
          ]),
        ),
      ].sort(),
    [meetingDirectoryQuery.data],
  );
  const meetingProfilesQuery = useUsersBatchQuery(meetingProfilePubkeys);
  const failureKind = contextQuery.isError
    ? projectContextFailureKind(contextQuery.error)
    : undefined;
  const verificationFailure = failureKind === "verification_failed";
  const result = verificationFailure ? undefined : contextQuery.data;
  const fatalError =
    contextQuery.isError && (!contextQuery.data || verificationFailure)
      ? contextQuery.error
      : undefined;
  const refreshError =
    contextQuery.isError && contextQuery.data && !verificationFailure
      ? contextQuery.error
      : undefined;
  const refreshMessage = refreshError
    ? projectContextErrorMessage(refreshError)
    : undefined;
  const relayDegraded = isRelayConnectionDegraded(relayConnection);
  const syncState: "live" | "refreshing" | "stale" | undefined = result
    ? relayDegraded || refreshMessage || liveStatus === "retrying"
      ? "stale"
      : contextQuery.isFetching || liveStatus === "connecting"
        ? "refreshing"
        : liveStatus === "live"
          ? "live"
          : undefined
    : undefined;
  const syncBadge = relayDegraded
    ? "Reconnecting"
    : liveStatus === "retrying"
      ? "Live reconnecting"
      : syncState === "stale"
        ? "Stale"
        : undefined;
  const syncMessage = !result
    ? undefined
    : relayDegraded
      ? `Showing verified Context revision ${result.context.contextRevision}. It may be stale while the Relay connection recovers.`
      : refreshMessage
        ? `Showing verified Context revision ${result.context.contextRevision}. The latest refresh failed: ${refreshMessage}`
        : liveStatus === "retrying"
          ? `Showing verified Context revision ${result.context.contextRevision} while the live update subscription reconnects.`
          : syncState === "refreshing"
            ? `Keeping verified Context revision ${result.context.contextRevision} visible while a new complete snapshot is verified.`
            : undefined;
  const projectViewObjects = React.useMemo(
    () =>
      projectViewQuery.data?.status === "ready"
        ? [...indexProjectViewObjects(projectViewQuery.data.view).values()]
        : undefined,
    [projectViewQuery.data],
  );
  const coordinateOptions = React.useMemo<ProjectContextCoordinateOption[]>(
    () =>
      buildProjectContextCoordinateOptions({
        projectViewObjects,
        documents: documentsQuery.data?.documents,
        meetings: meetingDirectoryQuery.data,
        profiles: meetingProfilesQuery.data?.profiles,
        visibleDetails: result?.coordinateDetails,
      }),
    [
      documentsQuery.data?.documents,
      meetingDirectoryQuery.data,
      meetingProfilesQuery.data?.profiles,
      projectViewObjects,
      result?.coordinateDetails,
    ],
  );
  const projectViewState = pickerSourceState({
    error: projectViewQuery.error,
    loading: projectViewQuery.isPending,
    ready: projectViewQuery.data?.status === "ready",
  });
  const documentsState = pickerSourceState({
    error: documentMetaQuery.error ?? documentsQuery.error,
    loading:
      documentMetaQuery.isPending ||
      Boolean(documentMetaQuery.data && documentsQuery.isPending),
    ready: Boolean(documentMetaQuery.data && documentsQuery.data),
  });
  const meetingsState = pickerSourceState({
    error: channelsQuery.error ?? meetingDirectoryQuery.error,
    loading:
      channelsQuery.isPending ||
      (meetingIds.length > 0 && meetingDirectoryQuery.isPending),
    ready:
      Boolean(channelsQuery.data) &&
      (meetingIds.length === 0 || Boolean(meetingDirectoryQuery.data)),
  });
  const allContext = isAllProjectContextQuery(appliedQuery);

  React.useEffect(() => {
    if (!result || !selection) return;
    const remainsVisible =
      selection.kind === "edge"
        ? result.edges.some((edge) => edge.edgeKey === selection.key)
        : result.coordinateDetails.some(
            (detail) => detail.coordinateKey === selection.key,
          ) ||
          result.edges.some((edge) =>
            edge.coordinateKeys.includes(selection.key),
          );
    if (!remainsVisible) onSelectionChange(null, { replace: true });
  }, [onSelectionChange, result, selection]);

  const closeInspector = React.useCallback(() => {
    const closingSelection = selection;
    onSelectionChange(null);
    if (!closingSelection) return;
    window.requestAnimationFrame(() => {
      focusProjectContextGraphTarget(closingSelection);
    });
  }, [onSelectionChange, selection]);

  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="project-context-screen"
    >
      <ProjectContextHeader
        onRefresh={() => void contextQuery.refetch()}
        refreshing={contextQuery.isFetching}
        result={result}
        syncBadge={syncBadge}
        syncState={syncState}
      />

      {syncState === "stale" && syncMessage && result ? (
        <div
          aria-live="polite"
          aria-atomic="true"
          className="flex items-start gap-2 border-b border-warning/30 bg-warning/10 px-4 py-2 text-xs text-muted-foreground"
          data-testid="project-context-stale-message"
          role="status"
        >
          <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{syncMessage}</span>
        </div>
      ) : syncState === "refreshing" && syncMessage ? (
        <div
          aria-live="polite"
          aria-atomic="true"
          className="border-b border-border/70 bg-muted/20 px-4 py-2 text-xs text-muted-foreground"
          data-testid="project-context-refreshing"
          role="status"
        >
          {syncMessage}
        </div>
      ) : null}

      <ProjectContextQueryBar
        appliedQuery={appliedQuery}
        coordinateOptions={coordinateOptions}
        documentsState={documentsState}
        meetingsState={meetingsState}
        onRun={onApplyQuery}
        projectViewState={projectViewState}
      />

      {contextQuery.isPending ? <ProjectContextLoadingState /> : null}
      {fatalError ? (
        <ProjectContextFailureState
          diagnostic={projectContextErrorMessage(fatalError)}
          kind={failureKind ?? projectContextFailureKind(fatalError)}
          onRetry={() => void contextQuery.refetch()}
          retrying={contextQuery.isFetching}
        />
      ) : null}
      {result && allContext && result.context.activeEdgeCount === 0 ? (
        <ProjectContextEmptyState />
      ) : null}
      {result && (!allContext || result.context.activeEdgeCount > 0) ? (
        <div className="flex min-h-0 flex-1 overflow-hidden">
          <ProjectContextGraphSlot
            focusSelectionRequest={focusSelectionRequest}
            onSelectionChange={onSelectionChange}
            result={result}
            selection={selection}
          />
          {selection ? (
            <ProjectContextInspector
              onClose={closeInspector}
              onFocusSelection={() => {
                focusProjectContextGraphTarget(selection);
                setFocusSelectionRequest((current) => current + 1);
              }}
              onOpenDocument={onOpenDocument}
              onOpenMeeting={onOpenMeeting}
              onOpenProjectView={onOpenProjectView}
              onSelect={(nextSelection) => onSelectionChange(nextSelection)}
              onShowIncident={(coordinate) =>
                onApplyQuery({ type: "incident", coordinate })
              }
              projectViewResult={projectViewQuery.data}
              result={result}
              selection={selection}
            />
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function InvalidProjectContextScreen({
  onResetRoute,
  routeError,
}: InvalidProjectContextScreenProps) {
  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="project-context-screen"
    >
      <ProjectContextHeader />
      <main
        className="flex min-h-0 flex-1 items-center justify-center p-6"
        data-testid="project-context-invalid-route"
      >
        <div className="max-w-lg text-center">
          <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-2xl border border-destructive/30 bg-destructive/10 text-destructive">
            <AlertTriangle className="h-5 w-5" />
          </div>
          <h1 className="mt-4 text-lg font-semibold">
            Project Context link is invalid
          </h1>
          <p className="mt-2 text-sm leading-relaxed text-muted-foreground">
            The query or selection in this link was rejected before Desktop
            contacted the trusted Project Context boundary.
          </p>
          <code className="mt-4 block rounded-lg border border-border/70 bg-muted/20 px-3 py-2 text-left text-xs text-muted-foreground">
            {routeError}
          </code>
          <Button
            className="mt-4"
            data-testid="project-context-reset-invalid-route"
            onClick={onResetRoute}
            size="sm"
            type="button"
            variant="outline"
          >
            Open All Context
          </Button>
        </div>
      </main>
    </div>
  );
}

/** Stable route surface for valid query state or rejected deep links. */
export function ProjectContextScreen(
  props: ValidProjectContextScreenProps | InvalidProjectContextScreenProps,
) {
  return "routeError" in props ? (
    <InvalidProjectContextScreen {...props} />
  ) : (
    <ValidProjectContextScreen {...props} />
  );
}
