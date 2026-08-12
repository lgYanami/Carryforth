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
  projectContextPickerSourceState,
  projectContextSemanticMeetingIds,
  projectContextSelectionRemainsVisible,
  projectContextSemanticToolStatus,
  projectContextWorkspaceStateEvent,
  type InvalidProjectContextScreenProps,
  type ValidProjectContextScreenProps,
} from "@/features/project-context/projectContextScreenModel";
import {
  buildProjectContextCoordinateOptions,
  type ProjectContextCoordinateOption,
} from "@/features/project-context/queryModel";
import {
  semanticErrorFromUnknown,
  semanticSourceFingerprint,
} from "@/features/project-context/semanticScreenModel";
import {
  buildProjectContextSemanticOverlay,
  semanticOverlayMatchesSubstrate,
  type ProjectContextSemanticOverlay,
} from "@/features/project-context/semanticOverlay";
import {
  nextSemanticAttemptToken,
  semanticOverlayEligible,
  semanticQueryRequiresAllContext,
  semanticResultMatchesIdentity,
  semanticSessionFreshness,
  semanticSessionReducer,
  createSemanticUiState,
} from "@/features/project-context/semanticSession";
import {
  submitSemanticQueryDraft,
  type SubmittedSemanticQueryDraft,
} from "@/features/project-context/semanticQueryModel";
import { focusProjectContextGraphTarget } from "@/features/project-context/focus";
import { isAllProjectContextQuery } from "@/features/project-context/routeState";
import {
  projectContextErrorMessage,
  projectContextFailureKind,
} from "@/features/project-context/state";
import {
  ProjectContextHeader,
  ProjectContextSyncBanner,
} from "@/features/project-context/ui/ProjectContextHeader";
import { ProjectContextInvalidRoute } from "@/features/project-context/ui/ProjectContextInvalidRoute";
import {
  ProjectContextWorkspace,
  type ProjectContextWorkspaceCanvasRenderContext,
  type ProjectContextWorkspacePaneRenderContext,
} from "@/features/project-context/ui/ProjectContextWorkspace";
import { ProjectContextWorkspaceCanvas } from "@/features/project-context/ui/ProjectContextWorkspaceCanvas";
import { ProjectContextWorkspacePane } from "@/features/project-context/ui/ProjectContextWorkspacePane";
import { projectContextWorkspaceSelectionKey } from "@/features/project-context/workspacePanelModel";
import {
  ALL_PROJECT_CONTEXT_QUERY,
  useProjectContextLiveSync,
  useProjectContextQuery,
} from "@/features/project-context/hooks";
import {
  useProjectContextWorkspaceAnnouncement,
  useProjectContextStructuralDraft,
} from "@/features/project-context/useProjectContextWorkspacePresentation";
import { useAppliedWorkspaceIdentity } from "@/features/communities/AppliedWorkspaceContext";
import { useProjectViewQuery } from "@/features/project-view/hooks";
import { indexProjectViewObjects } from "@/features/project-view/model";
import {
  queryProjectContextSemantic,
  SemanticProjectContextError,
  type SemanticProjectContextAcceptanceIdentity,
} from "@/shared/api/tauriProjectContextSemantic";
import {
  isRelayConnectionDegraded,
  useRelayConnection,
} from "@/shared/api/useRelayConnection";

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
  const [fitSemanticPathsRequest, setFitSemanticPathsRequest] =
    React.useState(0);
  const [structuralDraft, setStructuralDraft] =
    useProjectContextStructuralDraft(appliedQuery);
  const appliedWorkspace = useAppliedWorkspaceIdentity();
  const contextQuery = useProjectContextQuery(appliedQuery);
  const contextQueryFailure =
    contextQuery.error ?? contextQuery.failureReason ?? undefined;
  const failureKind = contextQueryFailure
    ? projectContextFailureKind(contextQueryFailure)
    : undefined;
  const verificationFailure = failureKind === "verification_failed";
  const routeResult = verificationFailure ? undefined : contextQuery.data;
  const fatalError =
    contextQueryFailure && (!contextQuery.data || verificationFailure)
      ? contextQueryFailure
      : undefined;
  const routeRefreshError =
    contextQueryFailure && contextQuery.data && !verificationFailure
      ? contextQueryFailure
      : undefined;
  const initialSemanticIdentity =
    React.useMemo<SemanticProjectContextAcceptanceIdentity>(
      () => ({
        communityKey: appliedWorkspace.communityKey,
        appliedWorkspaceToken: appliedWorkspace.appliedWorkspaceToken,
        callerPubkey: appliedWorkspace.callerPubkey,
        projectId: routeResult?.projectId ?? "",
        relayPubkey: routeResult?.relayPubkey ?? "",
      }),
      [appliedWorkspace, routeResult?.projectId, routeResult?.relayPubkey],
    );
  const [semanticState, semanticDispatch] = React.useReducer(
    semanticSessionReducer<ProjectContextSemanticOverlay>,
    createSemanticUiState<ProjectContextSemanticOverlay>(
      initialSemanticIdentity,
    ),
  );
  const semanticNeedsAllContext =
    semanticQueryRequiresAllContext(semanticState);
  const allContextQuery = useProjectContextQuery(ALL_PROJECT_CONTEXT_QUERY, {
    enabled: semanticNeedsAllContext,
  });
  const allContextQueryFailure =
    allContextQuery.error ?? allContextQuery.failureReason ?? undefined;
  const allContextFailureKind = allContextQueryFailure
    ? projectContextFailureKind(allContextQueryFailure)
    : undefined;
  const allContextResult =
    allContextFailureKind === "verification_failed"
      ? undefined
      : allContextQuery.data;
  const semanticIdentitySource = semanticNeedsAllContext
    ? (allContextResult ?? routeResult)
    : routeResult;
  const semanticIdentity =
    React.useMemo<SemanticProjectContextAcceptanceIdentity>(
      () => ({
        communityKey: appliedWorkspace.communityKey,
        appliedWorkspaceToken: appliedWorkspace.appliedWorkspaceToken,
        callerPubkey: appliedWorkspace.callerPubkey,
        projectId: semanticIdentitySource?.projectId ?? "",
        relayPubkey: semanticIdentitySource?.relayPubkey ?? "",
      }),
      [
        appliedWorkspace,
        semanticIdentitySource?.projectId,
        semanticIdentitySource?.relayPubkey,
      ],
    );
  React.useEffect(() => {
    semanticDispatch({ type: "boundary_changed", identity: semanticIdentity });
  }, [semanticIdentity]);
  const observedCapabilityResult = semanticNeedsAllContext
    ? (allContextResult ?? routeResult)
    : routeResult;
  const substrateIdentityMismatch = Boolean(
    semanticNeedsAllContext &&
      allContextResult &&
      routeResult &&
      (allContextResult.projectId !== routeResult.projectId ||
        allContextResult.relayPubkey !== routeResult.relayPubkey),
  );
  const semanticSafetyFailure =
    substrateIdentityMismatch ||
    [failureKind, allContextFailureKind].some(
      (kind) =>
        kind === "restricted" ||
        kind === "verification_failed" ||
        kind === "unsupported",
    );
  const semanticAvailable =
    !semanticSafetyFailure &&
    observedCapabilityResult?.context.semanticQueryAvailable === true &&
    semanticIdentity.projectId.length > 0 &&
    semanticIdentity.relayPubkey.length > 0;
  const trustedActive =
    semanticState.active &&
    semanticAvailable &&
    semanticResultMatchesIdentity(
      semanticState.active.verifiedDisplayResult,
      semanticIdentity,
    )
      ? semanticState.active
      : null;
  const result = trustedActive ? allContextResult : routeResult;
  const semanticMeetingIds = React.useMemo(() => {
    const submittedDrafts: SubmittedSemanticQueryDraft[] = [];
    if (trustedActive) submittedDrafts.push(trustedActive.submittedDraft);
    if (
      semanticState.attempt.status === "running" ||
      semanticState.attempt.status === "pairing"
    ) {
      submittedDrafts.push(semanticState.attempt.submitted);
    }
    return projectContextSemanticMeetingIds({
      coordinateDetails: allContextResult?.coordinateDetails,
      enabled: semanticNeedsAllContext,
      submittedDrafts,
    });
  }, [
    allContextResult?.coordinateDetails,
    semanticNeedsAllContext,
    semanticState.attempt,
    trustedActive,
  ]);
  const liveStatus = useProjectContextLiveSync(
    semanticNeedsAllContext ? (allContextResult ?? routeResult) : routeResult,
    semanticMeetingIds,
  );
  const relayConnection = useRelayConnection();
  const semanticTransportUncertain =
    relayConnection !== "connected" ||
    liveStatus !== "live" ||
    (semanticNeedsAllContext && allContextQuery.isError);
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
  const displayedFatalError = trustedActive
    ? allContextQueryFailure && !allContextResult
      ? allContextQueryFailure
      : undefined
    : fatalError;
  const displayedFailureKind = displayedFatalError
    ? projectContextFailureKind(displayedFatalError)
    : undefined;
  const displayedQueryPending = trustedActive
    ? allContextQuery.isPending
    : contextQuery.isPending;
  const displayedRefreshError = semanticNeedsAllContext
    ? allContextQueryFailure
      ? allContextQueryFailure
      : undefined
    : routeRefreshError;
  const refreshMessage = displayedRefreshError
    ? projectContextErrorMessage(displayedRefreshError)
    : undefined;
  const relayDegraded = isRelayConnectionDegraded(relayConnection);
  const displayedQueryFetching = semanticNeedsAllContext
    ? allContextQuery.isFetching
    : contextQuery.isFetching;
  const syncState: "live" | "refreshing" | "stale" | undefined = result
    ? relayDegraded || refreshMessage || liveStatus === "retrying"
      ? "stale"
      : displayedQueryFetching || liveStatus === "connecting"
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
  const workspaceStateEvent = projectContextWorkspaceStateEvent({
    failureKind: displayedFatalError
      ? (displayedFailureKind ?? "error")
      : undefined,
    pending: displayedQueryPending,
    projectId: semanticIdentity.projectId,
    revision: result?.context.contextRevision,
    syncMessage,
    syncState,
  });
  const {
    announcement: workspaceAnnouncement,
    announceCleared: announceSemanticCleared,
    announceEvent: announceWorkspaceEvent,
  } = useProjectContextWorkspaceAnnouncement({
    active: trustedActive,
    attempt: semanticState.attempt,
    selection,
    workspaceStateEvent,
  });
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
  const projectViewState = projectContextPickerSourceState({
    error: projectViewQuery.error,
    loading: projectViewQuery.isPending,
    ready: projectViewQuery.data?.status === "ready",
  });
  const documentsState = projectContextPickerSourceState({
    error: documentMetaQuery.error ?? documentsQuery.error,
    loading:
      documentMetaQuery.isPending ||
      Boolean(documentMetaQuery.data && documentsQuery.isPending),
    ready: Boolean(documentMetaQuery.data && documentsQuery.data),
  });
  const meetingsState = projectContextPickerSourceState({
    error: channelsQuery.error ?? meetingDirectoryQuery.error,
    loading:
      channelsQuery.isPending ||
      (meetingIds.length > 0 && meetingDirectoryQuery.isPending),
    ready:
      Boolean(channelsQuery.data) &&
      (meetingIds.length === 0 || Boolean(meetingDirectoryQuery.data)),
  });
  const allContext = isAllProjectContextQuery(appliedQuery);
  const retryingPairRef = React.useRef<string | null>(null);
  const sourceFingerprintRef = React.useRef<{
    requestId: string;
    fingerprint: string;
  } | null>(null);
  const semanticAttemptInFlight =
    semanticState.attempt.status === "running" ||
    semanticState.attempt.status === "pairing";
  const semanticActivityPresent =
    semanticState.active !== null || semanticAttemptInFlight;

  React.useEffect(() => {
    if (semanticAvailable || !semanticActivityPresent) return;
    semanticDispatch({ type: "capability_lost" });
  }, [semanticActivityPresent, semanticAvailable]);

  React.useEffect(() => {
    if (!trustedActive) return;
    semanticDispatch({
      type: "transport_observed",
      state: semanticTransportUncertain ? "uncertain" : "live",
    });
  }, [semanticTransportUncertain, trustedActive]);

  React.useEffect(() => {
    if (!trustedActive || !allContextResult) return;
    semanticDispatch({
      type: "topology_observed",
      substrateRevision: allContextResult.context.contextRevision,
    });
  }, [allContextResult, trustedActive]);

  React.useEffect(() => {
    const attempt = semanticState.attempt;
    if (attempt.status !== "pairing") return;
    if (allContextQuery.isError && !allContextQuery.isFetching) {
      semanticDispatch({
        type: "native_failed",
        token: attempt.token,
        error: semanticErrorFromUnknown(allContextQuery.error),
      });
      return;
    }
    if (!allContextResult) return;
    const substrateRevision = allContextResult.context.contextRevision;
    const resultRevision = attempt.verifiedDisplayResult.projectContextRevision;
    if (substrateRevision < resultRevision) {
      if (allContextQuery.isFetching) return;
      const retryKey = `${attempt.token}:${substrateRevision}`;
      if (retryingPairRef.current !== retryKey) {
        retryingPairRef.current = retryKey;
        void allContextQuery.refetch();
        return;
      }
      semanticDispatch({
        type: "native_failed",
        token: attempt.token,
        error: new SemanticProjectContextError({
          code: "conflict",
          message:
            "All Context has not reached the verified semantic result revision. Refresh and run the query again.",
          retryable: true,
        }),
      });
      return;
    }
    retryingPairRef.current = null;
    const joined = buildProjectContextSemanticOverlay(
      attempt.verifiedDisplayResult,
      allContextResult,
    );
    semanticDispatch({
      type: "pairing_observed",
      token: attempt.token,
      substrateRevision,
      join: joined.ok
        ? { status: "valid", overlay: joined.overlay }
        : {
            status: "invalid",
            message: `Verified semantic paths no longer match All Context (${joined.reason}).`,
          },
    });
  }, [
    allContextQuery.error,
    allContextQuery.isError,
    allContextQuery.isFetching,
    allContextQuery.refetch,
    allContextResult,
    semanticState.attempt,
  ]);

  const activeSourceFingerprint = React.useMemo(() => {
    if (!trustedActive || !allContextResult) return undefined;
    return semanticSourceFingerprint(
      trustedActive,
      allContextResult,
      coordinateOptions,
    );
  }, [allContextResult, coordinateOptions, trustedActive]);
  React.useEffect(() => {
    const active = trustedActive;
    if (!active || !activeSourceFingerprint) {
      sourceFingerprintRef.current = null;
      return;
    }
    if (sourceFingerprintRef.current?.requestId !== active.requestId) {
      sourceFingerprintRef.current = {
        requestId: active.requestId,
        fingerprint: activeSourceFingerprint,
      };
      return;
    }
    semanticDispatch({
      type: "source_refresh_observed",
      fingerprintMatches:
        sourceFingerprintRef.current.fingerprint === activeSourceFingerprint,
    });
  }, [activeSourceFingerprint, trustedActive]);

  const semanticAttemptRef = React.useRef(0);
  const semanticDraft = semanticState.draft;
  const semanticGeneration = semanticState.generation;
  React.useEffect(() => {
    semanticAttemptRef.current = Math.max(
      semanticAttemptRef.current,
      semanticGeneration,
    );
  }, [semanticGeneration]);
  const runSemanticQuery = React.useCallback(() => {
    if (!semanticAvailable) return;
    let submitted: SubmittedSemanticQueryDraft;
    try {
      submitted = submitSemanticQueryDraft(semanticDraft);
    } catch (error) {
      const token = Math.max(
        ++semanticAttemptRef.current,
        nextSemanticAttemptToken({ generation: semanticGeneration }),
      );
      semanticAttemptRef.current = token;
      semanticDispatch({
        type: "run_started",
        token,
        submitted: {
          problem: semanticDraft.problem,
          initialCoordinates: semanticDraft.initialCoordinates,
          contextCoordinates: semanticDraft.contextCoordinates,
        },
      });
      semanticDispatch({
        type: "native_failed",
        token,
        error: semanticErrorFromUnknown(error),
      });
      return;
    }
    const token = Math.max(
      ++semanticAttemptRef.current,
      nextSemanticAttemptToken({ generation: semanticGeneration }),
    );
    semanticAttemptRef.current = token;
    semanticDispatch({ type: "run_started", token, submitted });
    void queryProjectContextSemantic(
      {
        communityKey: semanticIdentity.communityKey,
        appliedWorkspaceToken: semanticIdentity.appliedWorkspaceToken,
        problem: submitted.problem,
        initialCoordinates: submitted.initialCoordinates,
        contextCoordinates: submitted.contextCoordinates,
      },
      semanticIdentity,
    ).then(
      (semanticResult) =>
        semanticDispatch({
          type: "native_succeeded",
          token,
          result: semanticResult,
        }),
      (error) =>
        semanticDispatch({
          type: "native_failed",
          token,
          error: semanticErrorFromUnknown(error),
        }),
    );
  }, [semanticAvailable, semanticDraft, semanticGeneration, semanticIdentity]);
  const cancelSemanticQuery = React.useCallback(() => {
    semanticAttemptRef.current += 1;
    announceSemanticCleared();
    semanticDispatch({ type: "cancel" });
  }, [announceSemanticCleared]);

  const semanticStructuralJoinValid = Boolean(
    trustedActive &&
      result &&
      semanticOverlayMatchesSubstrate(trustedActive.overlay, result),
  );
  const semanticOverlay =
    trustedActive &&
    result &&
    semanticOverlayEligible(
      semanticState,
      result.context.contextRevision,
      semanticStructuralJoinValid,
    )
      ? trustedActive.overlay
      : null;
  const semanticTopologyAdvanced = Boolean(
    trustedActive &&
      (semanticState.freshness.topology === "advanced" ||
        allContextResult?.context.contextRevision !==
          trustedActive.projectContextRevision),
  );
  const semanticFreshness =
    trustedActive && (semanticTopologyAdvanced || semanticTransportUncertain)
      ? "stale"
      : semanticSessionFreshness(semanticState);
  const displayedAllContext = result
    ? isAllProjectContextQuery(result.query)
    : allContext;

  React.useEffect(() => {
    if (!result || !selection) return;
    if (!projectContextSelectionRemainsVisible(result, selection)) {
      onSelectionChange(null, { replace: true });
    }
  }, [onSelectionChange, result, selection]);

  const closeInspector = React.useCallback(() => {
    onSelectionChange(null);
  }, [onSelectionChange]);

  const workspaceSelectionKey = selection
    ? projectContextWorkspaceSelectionKey(selection)
    : null;
  const semanticToolStatus = projectContextSemanticToolStatus({
    active: trustedActive !== null,
    inFlight: semanticAttemptInFlight,
    stale: semanticFreshness === "stale",
  });

  const retryDisplayedContext = React.useCallback(() => {
    void (trustedActive ? allContextQuery.refetch() : contextQuery.refetch());
  }, [allContextQuery.refetch, contextQuery.refetch, trustedActive]);

  const renderWorkspaceCanvas = React.useCallback(
    (canvas: ProjectContextWorkspaceCanvasRenderContext) => (
      <ProjectContextWorkspaceCanvas
        canvas={canvas}
        displayedAllContext={displayedAllContext}
        failure={displayedFatalError}
        failureKind={displayedFailureKind}
        fitSemanticPathsRequest={fitSemanticPathsRequest}
        focusSelectionRequest={focusSelectionRequest}
        onClearSemanticResult={cancelSemanticQuery}
        onRetry={retryDisplayedContext}
        onSelectionChange={onSelectionChange}
        pending={displayedQueryPending}
        result={result}
        retrying={
          trustedActive ? allContextQuery.isFetching : contextQuery.isFetching
        }
        selection={selection}
        semanticFreshness={semanticFreshness}
        semanticOverlay={semanticOverlay}
        semanticSessionOverlay={trustedActive?.overlay ?? null}
      />
    ),
    [
      allContextQuery.isFetching,
      cancelSemanticQuery,
      contextQuery.isFetching,
      displayedAllContext,
      displayedFailureKind,
      displayedFatalError,
      displayedQueryPending,
      fitSemanticPathsRequest,
      focusSelectionRequest,
      onSelectionChange,
      result,
      retryDisplayedContext,
      selection,
      semanticFreshness,
      semanticOverlay,
      trustedActive,
    ],
  );

  const renderWorkspacePane = React.useCallback(
    (
      tool: "structure" | "semantic" | "details",
      pane: ProjectContextWorkspacePaneRenderContext,
    ) => (
      <ProjectContextWorkspacePane
        activeSemantic={trustedActive}
        appliedQuery={appliedQuery}
        coordinateOptions={coordinateOptions}
        onApplyQuery={onApplyQuery}
        onAnnouncement={announceWorkspaceEvent}
        onCancelSemantic={cancelSemanticQuery}
        onFitSemantic={() => {
          setFitSemanticPathsRequest((current) => current + 1);
          pane.closeModalForViewportAction();
        }}
        onFocusSelection={() => {
          setFocusSelectionRequest((current) => current + 1);
          pane.closeModalForViewportAction(() => {
            if (selection) focusProjectContextGraphTarget(selection);
          });
        }}
        onOpenDocument={onOpenDocument}
        onOpenMeeting={onOpenMeeting}
        onOpenProjectView={onOpenProjectView}
        onRunSemantic={runSemanticQuery}
        onSelectionChange={(nextSelection) => onSelectionChange(nextSelection)}
        onSemanticDraftChange={(draft) =>
          semanticDispatch({ type: "draft_changed", draft })
        }
        onStructuralDraftChange={setStructuralDraft}
        pickerStates={{
          documents: documentsState,
          meetings: meetingsState,
          projectView: projectViewState,
        }}
        projectViewResult={projectViewQuery.data}
        result={result}
        selection={selection}
        semanticAttempt={semanticState.attempt}
        semanticAvailable={semanticAvailable}
        semanticDraft={semanticState.draft}
        semanticFreshness={semanticFreshness}
        semanticNeedsAllContext={semanticNeedsAllContext}
        semanticOverlay={semanticOverlay}
        semanticTopologyAdvanced={semanticTopologyAdvanced}
        structuralDraft={structuralDraft}
        tool={tool}
      />
    ),
    [
      appliedQuery,
      announceWorkspaceEvent,
      cancelSemanticQuery,
      coordinateOptions,
      documentsState,
      meetingsState,
      onApplyQuery,
      onOpenDocument,
      onOpenMeeting,
      onOpenProjectView,
      onSelectionChange,
      projectViewQuery.data,
      projectViewState,
      result,
      runSemanticQuery,
      selection,
      semanticAvailable,
      semanticFreshness,
      semanticNeedsAllContext,
      semanticOverlay,
      semanticState.attempt,
      semanticState.draft,
      semanticTopologyAdvanced,
      setStructuralDraft,
      structuralDraft,
      trustedActive,
    ],
  );

  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-testid="project-context-screen"
    >
      <ProjectContextHeader
        onRefresh={() =>
          void (semanticNeedsAllContext
            ? allContextQuery.refetch()
            : contextQuery.refetch())
        }
        refreshing={
          semanticNeedsAllContext
            ? allContextQuery.isFetching
            : contextQuery.isFetching
        }
        result={result}
        syncBadge={syncBadge}
        syncState={syncState}
      />

      <ProjectContextSyncBanner message={syncMessage} state={syncState} />
      <ProjectContextWorkspace
        announcement={workspaceAnnouncement}
        detailsUnavailableReason="Select a Coordinate or Context Edge to inspect details."
        onCloseSelection={closeInspector}
        onRestoreCanvasFocus={() =>
          document
            .querySelector<HTMLElement>(
              '[data-testid="project-context-graph-slot"]',
            )
            ?.focus({ preventScroll: true })
        }
        onRestoreGraphTargetFocus={focusProjectContextGraphTarget}
        renderCanvas={renderWorkspaceCanvas}
        renderPane={renderWorkspacePane}
        selectionKey={workspaceSelectionKey}
        semanticStatus={semanticToolStatus}
      />
    </div>
  );
}

/** Stable route surface for valid query state or rejected deep links. */
export function ProjectContextScreen(
  props: ValidProjectContextScreenProps | InvalidProjectContextScreenProps,
) {
  return "routeError" in props ? (
    <ProjectContextInvalidRoute {...props} />
  ) : (
    <ValidProjectContextScreen {...props} />
  );
}
