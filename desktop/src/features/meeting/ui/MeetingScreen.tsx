import * as React from "react";
import { AlertTriangle, ClipboardList, RefreshCw } from "lucide-react";

import { useAppShell } from "@/app/AppShellContext";
import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
  useMeetingActivities,
  useMeetingSnapshot,
  useMeetingSpeeches,
} from "@/features/meeting/hooks";
import { useMeetingBoardDraft } from "@/features/meeting/useMeetingBoardDraft";
import { useMeetingAuthority } from "@/features/meeting/useMeetingAuthority";
import { useResizableMeetingBoardWidth } from "@/features/meeting/useResizableMeetingBoardWidth";
import { useCommunities } from "@/features/communities/useCommunities";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingActivity,
  MeetingSnapshot,
  MeetingSpeech,
} from "@/shared/api/tauriMeetings";
import {
  ensureMeetingActionRenewal,
  ensureMeetingHumanGrantRenewal,
} from "@/shared/api/tauriMeetings";
import { setVisibleChannel } from "@/shared/api/relayClient";
import { useIdentityQuery } from "@/shared/api/hooks";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { useMediaBreakpoint } from "@/shared/hooks/use-mobile";
import { usePreviewFeatureWarning } from "@/shared/features";
import { copyTextToClipboard } from "@/shared/lib/clipboard";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/shared/ui/sheet";
import { ViewLoadingFallback } from "@/shared/ui/ViewLoadingFallback";
import { MeetingBoardPanel } from "./MeetingBoardPanel";
import { MeetingActivityPanel } from "./MeetingActivityPanel";
import { MeetingFloorDock } from "./MeetingFloorDock";
import { MeetingHeader } from "./MeetingHeader";
import { MeetingParticipantsPanel } from "./MeetingParticipantsPanel";
import { MeetingSpeechTimeline } from "./MeetingSpeechTimeline";
import { MeetingTerminalSummary } from "./MeetingTerminalSummary";

const EMPTY_PROFILES: Record<string, UserProfileSummary> = {};

function profileName(
  pubkey: string | null,
  profiles: Record<string, UserProfileSummary>,
): string | null {
  if (!pubkey) return null;
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

function meetingStatusText(
  snapshot: MeetingSnapshot,
  profiles: Record<string, UserProfileSummary>,
): string {
  if (snapshot.lifecycle === "closed") {
    return snapshot.end?.actionsAttested
      ? "The host confirmed that meeting actions were recorded."
      : "The host closed the meeting after reaching its goal.";
  }
  if (snapshot.lifecycle === "aborted") {
    return snapshot.end?.reason
      ? `Meeting aborted: ${snapshot.end.reason}`
      : "The meeting ended without a successful conclusion.";
  }
  if (snapshot.lifecycle === "finalizing_actions") {
    const condition = snapshot.action?.condition;
    return condition === "blocked"
      ? "The host's action-recording window is blocked and requires recovery."
      : "The host is recording the final Board actions in the relevant systems.";
  }
  if (snapshot.host?.boardControl.phase === "board_pending") {
    return "The host must review the Meeting Board before arranging the next speaker.";
  }
  if (snapshot.phase === "granted") {
    const speaker = profileName(snapshot.currentSpeakerPubkey, profiles);
    return speaker
      ? `${speaker} has the floor.`
      : "A participant has the floor.";
  }
  if (snapshot.phase === "offered") {
    const target = profileName(snapshot.currentOfferPubkey, profiles);
    return target
      ? `The floor has been offered to ${target}.`
      : "The next participant is acknowledging the floor offer.";
  }
  if (snapshot.phase === "moderator_control") {
    return "The host is deciding who speaks next.";
  }
  if (snapshot.lifecycle === "initializing") {
    return "The Relay is preparing the authoritative Meeting state.";
  }
  return "The host currently controls the meeting floor.";
}

function MeetingLoadState({
  message,
  onRetry,
  title,
}: {
  message: string;
  onRetry?: () => void;
  title: string;
}) {
  return (
    <div
      className="flex min-h-0 flex-1 flex-col"
      data-testid="meeting-load-state"
    >
      <TopChromeInsetHeader flush>
        <div className="flex h-12 items-center px-5 text-sm font-semibold">
          Meeting
        </div>
      </TopChromeInsetHeader>
      <div className="flex flex-1 items-center justify-center p-6">
        <div className="max-w-md rounded-xl border bg-card p-6 text-center shadow-xs">
          <AlertTriangle className="mx-auto size-6 text-amber-500" />
          <h1 className="mt-3 text-base font-semibold">{title}</h1>
          <p className="mt-2 text-sm text-muted-foreground">{message}</p>
          {onRetry ? (
            <Button className="mt-4" onClick={onRetry} variant="outline">
              <RefreshCw className="size-4" />
              Retry
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

export function MeetingScreen({ meetingId }: { meetingId: string }) {
  usePreviewFeatureWarning("meeting");
  const { activeCommunity } = useCommunities();
  const { goChannel } = useAppNavigation();
  const { markChannelRead } = useAppShell();
  const identityQuery = useIdentityQuery();
  const snapshotQuery = useMeetingSnapshot(meetingId);
  const snapshot =
    snapshotQuery.data?.status === "ready" ? snapshotQuery.data.snapshot : null;
  const meetingAuthority = useMeetingAuthority({
    hasVerifiedSnapshot: snapshot !== null,
    readError: Boolean(snapshotQuery.error),
    refetch: snapshotQuery.refetch,
    scopeKey: `${activeCommunity?.id ?? "no-community"}:${meetingId}`,
  });
  const boardWidth = useResizableMeetingBoardWidth(activeCommunity?.id);
  const normalizedPubkey = identityQuery.data?.pubkey.toLowerCase();
  const currentParticipant = snapshot?.participants.find(
    (participant) => participant.pubkey === normalizedPubkey,
  );
  const activeGrant = snapshot?.floor?.grant ?? null;
  const humanAction =
    snapshot?.policy === "moderated-board-actions-v3" &&
    snapshot.lifecycle === "finalizing_actions" &&
    snapshot.action?.condition === "runnable" &&
    snapshot.action.terminalStatus === null &&
    normalizedPubkey === snapshot.moderatorPubkey &&
    currentParticipant?.participantType === "human"
      ? snapshot.action
      : null;
  const humanGrant =
    snapshot?.lifecycle === "active" &&
    snapshot.phase === "granted" &&
    currentParticipant?.participantType === "human" &&
    activeGrant?.holderPubkey === normalizedPubkey
      ? activeGrant
      : null;
  const humanGrantId = humanGrant?.grantId ?? null;
  const [grantRenewalFailure, setGrantRenewalFailure] = React.useState<{
    grantId: string;
    message: string;
  } | null>(null);
  React.useEffect(() => {
    if (!humanAction || meetingAuthority.status !== "current") return;
    void ensureMeetingActionRenewal({
      meetingId,
      actionRunId: humanAction.actionRunId,
      actionWindowEpoch: humanAction.actionWindowEpoch,
      boardEventId: humanAction.boardEventId,
    }).catch((error) => {
      console.error("Failed to retain the Human Meeting Action lease", error);
    });
  }, [humanAction, meetingAuthority.status, meetingId]);
  const refetchSnapshot = snapshotQuery.refetch;
  React.useEffect(() => {
    if (!humanGrantId || meetingAuthority.status !== "current") {
      setGrantRenewalFailure(null);
      return;
    }
    let disposed = false;
    let retryTimer: number | null = null;
    const ensureRenewal = async () => {
      try {
        await ensureMeetingHumanGrantRenewal({
          meetingId,
          grantId: humanGrantId,
        });
        if (!disposed) setGrantRenewalFailure(null);
      } catch (error) {
        if (disposed) return;
        const message = error instanceof Error ? error.message : String(error);
        console.error("Failed to retain the Human Meeting Grant", error);
        setGrantRenewalFailure({ grantId: humanGrantId, message });
        void refetchSnapshot();
        retryTimer = window.setTimeout(() => void ensureRenewal(), 2_000);
      }
    };
    void ensureRenewal();
    return () => {
      disposed = true;
      if (retryTimer !== null) window.clearTimeout(retryTimer);
    };
  }, [humanGrantId, meetingAuthority.status, meetingId, refetchSnapshot]);
  const boardEditable = Boolean(
    snapshot &&
      snapshot.lifecycle === "active" &&
      normalizedPubkey === snapshot.moderatorPubkey &&
      currentParticipant?.participantType === "human" &&
      snapshot.host?.boardControl.phase === "board_pending",
  );
  const boardDraft = useMeetingBoardDraft({
    editable: boardEditable,
    snapshot,
  });
  const boardPanelIsOverlay = useMediaBreakpoint(1280);
  const [activitySheetState, setActivitySheetState] = React.useState({
    meetingId,
    open: false,
  });
  const activitySheetOpen =
    activitySheetState.meetingId === meetingId && activitySheetState.open;
  const setActivitySheetOpen = React.useCallback(
    (open: boolean) => setActivitySheetState({ meetingId, open }),
    [meetingId],
  );
  const [participantSheetState, setParticipantSheetState] = React.useState({
    meetingId,
    open: false,
  });
  const participantSheetOpen =
    participantSheetState.meetingId === meetingId && participantSheetState.open;
  const setParticipantSheetOpen = React.useCallback(
    (open: boolean) => setParticipantSheetState({ meetingId, open }),
    [meetingId],
  );
  const [abortDialogState, setAbortDialogState] = React.useState({
    meetingId,
    open: false,
  });
  const abortDialogOpen =
    abortDialogState.meetingId === meetingId && abortDialogState.open;
  const setAbortDialogOpen = React.useCallback(
    (open: boolean) => setAbortDialogState({ meetingId, open }),
    [meetingId],
  );
  const [boardSheetState, setBoardSheetState] = React.useState({
    meetingId,
    open: false,
  });
  const boardSheetOpen =
    boardSheetState.meetingId === meetingId && boardSheetState.open;
  const setBoardSheetOpen = React.useCallback(
    (open: boolean) => setBoardSheetState({ meetingId, open }),
    [meetingId],
  );
  const [wideBoardState, setWideBoardState] = React.useState({
    meetingId,
    open: true,
  });
  const wideBoardOpen =
    wideBoardState.meetingId === meetingId ? wideBoardState.open : true;
  const setWideBoardOpen = React.useCallback(
    (open: boolean) => setWideBoardState({ meetingId, open }),
    [meetingId],
  );
  const autoOpenedBoardKeyRef = React.useRef<string | null>(null);

  React.useEffect(() => {
    if (!boardEditable || !boardDraft.controlToken) return;
    const autoOpenKey = `${meetingId}:${boardDraft.controlToken}:${boardPanelIsOverlay ? "overlay" : "wide"}`;
    if (autoOpenedBoardKeyRef.current === autoOpenKey) return;
    autoOpenedBoardKeyRef.current = autoOpenKey;
    if (boardPanelIsOverlay) {
      setBoardSheetOpen(true);
    } else {
      setWideBoardOpen(true);
    }
  }, [
    boardDraft.controlToken,
    boardEditable,
    boardPanelIsOverlay,
    meetingId,
    setBoardSheetOpen,
    setWideBoardOpen,
  ]);
  const speechesQuery = useMeetingSpeeches({
    meetingId,
    enabled: snapshot !== null,
  });
  const activitiesQuery = useMeetingActivities({
    meetingId,
    enabled: snapshot !== null && activitySheetOpen,
  });
  const participantPubkeys = React.useMemo(
    () =>
      snapshot
        ? [
            ...new Set([
              ...snapshot.participants.map((participant) => participant.pubkey),
              ...(snapshot.end ? [snapshot.end.endedBy] : []),
            ]),
          ]
        : [],
    [snapshot],
  );
  const profilesQuery = useUsersBatchQuery(participantPubkeys, {
    enabled: participantPubkeys.length > 0,
  });
  const profiles = profilesQuery.data?.profiles ?? EMPTY_PROFILES;
  const speeches = React.useMemo(() => {
    const byId = new Map<string, MeetingSpeech>();
    for (const page of speechesQuery.data?.pages ?? []) {
      for (const speech of page.speeches) byId.set(speech.eventId, speech);
    }
    return [...byId.values()].sort(
      (left, right) =>
        left.speechRevision - right.speechRevision ||
        left.eventId.localeCompare(right.eventId),
    );
  }, [speechesQuery.data?.pages]);
  const activities = React.useMemo(() => {
    const ordered: MeetingActivity[] = [];
    const seenIds = new Set<string>();
    for (const page of activitiesQuery.data?.pages ?? []) {
      for (const activity of page.activities) {
        if (seenIds.has(activity.activityId)) continue;
        seenIds.add(activity.activityId);
        ordered.push(activity);
      }
    }
    return ordered;
  }, [activitiesQuery.data?.pages]);

  React.useEffect(() => {
    setVisibleChannel(meetingId);
    return () => setVisibleChannel(null);
  }, [meetingId]);

  React.useEffect(() => {
    if (!snapshot?.latestSpeechAt) return;
    markChannelRead(
      meetingId,
      new Date(snapshot.latestSpeechAt * 1_000).toISOString(),
    );
  }, [markChannelRead, meetingId, snapshot?.latestSpeechAt]);

  if (snapshotQuery.isPending) {
    return <ViewLoadingFallback includeHeader kind="channel" />;
  }
  if (snapshotQuery.error && !snapshotQuery.data) {
    return (
      <MeetingLoadState
        message={
          snapshotQuery.error instanceof Error
            ? snapshotQuery.error.message
            : "The Meeting could not be loaded."
        }
        onRetry={() => void snapshotQuery.refetch()}
        title="Could not verify this Meeting"
      />
    );
  }
  if (snapshotQuery.data.status === "unsupported_relay") {
    return (
      <MeetingLoadState
        message="This Community Relay does not advertise Meeting V2 read support. The room remains isolated from normal Channel messaging."
        title="Meeting V2 is not supported"
      />
    );
  }
  if (snapshotQuery.data.status === "forbidden") {
    return (
      <MeetingLoadState
        message="Only the Meeting's frozen roster can read its authoritative state."
        title="You are not a participant"
      />
    );
  }
  if (snapshotQuery.data.status === "not_found") {
    return (
      <MeetingLoadState
        message="The Relay could not find a signed Create event for this Meeting."
        title="Meeting not found"
      />
    );
  }
  if (snapshotQuery.data.status === "unsupported_protocol") {
    return (
      <MeetingLoadState
        message={`Desktop cannot safely interpret protocol ${snapshotQuery.data.schema_version ?? "unknown"} / ${snapshotQuery.data.policy ?? "unknown"}. The room remains read-only and never falls back to a normal Channel.`}
        title="Meeting compatibility required"
      />
    );
  }

  const readySnapshot = snapshotQuery.data.snapshot;
  const terminal =
    readySnapshot.lifecycle === "closed" ||
    readySnapshot.lifecycle === "aborted";
  const statusText = meetingStatusText(readySnapshot, profiles);
  const sourceChannelId = readySnapshot.sourceChannelId;
  const boardOpen = boardPanelIsOverlay ? boardSheetOpen : wideBoardOpen;
  const boardTrigger = (
    <Button
      aria-controls={
        boardPanelIsOverlay
          ? "meeting-board-overlay-panel"
          : "meeting-board-wide-panel"
      }
      aria-expanded={boardOpen}
      data-testid="meeting-board-trigger"
      onClick={
        boardPanelIsOverlay ? undefined : () => setWideBoardOpen(!wideBoardOpen)
      }
      size="sm"
      title={boardOpen ? "Hide Meeting board" : "Show Meeting board"}
      variant="outline"
    >
      <ClipboardList className="size-4" />
      Board
    </Button>
  );
  const boardControl = boardPanelIsOverlay ? (
    <Sheet onOpenChange={setBoardSheetOpen} open={boardSheetOpen}>
      <SheetTrigger asChild>{boardTrigger}</SheetTrigger>
      <SheetContent
        className="p-0 sm:max-w-xl"
        id="meeting-board-overlay-panel"
      >
        <SheetHeader className="sr-only">
          <SheetTitle>Meeting board</SheetTitle>
          <SheetDescription>
            The current Board maintained by the host.
          </SheetDescription>
        </SheetHeader>
        <MeetingBoardPanel
          board={readySnapshot.board}
          className="h-full"
          editor={
            boardEditable
              ? {
                  disabled: !meetingAuthority.authorityAvailable,
                  onChange: boardDraft.setValue,
                  value: boardDraft.value,
                }
              : undefined
          }
          onDismissStaleDraft={boardDraft.dismissStale}
          staleDraft={boardDraft.stale}
        />
      </SheetContent>
    </Sheet>
  ) : (
    boardTrigger
  );
  const canAbort = Boolean(
    (readySnapshot.lifecycle === "active" ||
      readySnapshot.lifecycle === "finalizing_actions") &&
      normalizedPubkey === readySnapshot.moderatorPubkey &&
      currentParticipant?.participantType === "human",
  );
  const openTerminalOutcome = () => {
    const summary = document.getElementById("meeting-terminal-outcome");
    summary?.scrollIntoView({ behavior: "smooth", block: "nearest" });
    summary?.focus({ preventScroll: true });
  };

  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-meeting-lifecycle={readySnapshot.lifecycle}
      data-meeting-authority={meetingAuthority.status}
      data-testid="meeting-screen"
    >
      <TopChromeInsetHeader flush>
        <MeetingHeader
          abortDisabled={
            !meetingAuthority.authorityAvailable || !readySnapshot.host
          }
          boardControl={boardControl}
          canAbort={canAbort}
          onAbort={() => setAbortDialogOpen(true)}
          onCopyLink={() =>
            copyTextToClipboard(window.location.href, "Meeting link copied")
          }
          onOpenActivity={() => setActivitySheetOpen(true)}
          onOpenOutcome={openTerminalOutcome}
          onOpenParticipants={() => setParticipantSheetOpen(true)}
          onOpenSource={
            sourceChannelId ? () => void goChannel(sourceChannelId) : undefined
          }
          profiles={profiles}
          snapshot={readySnapshot}
        />
      </TopChromeInsetHeader>

      <Sheet onOpenChange={setParticipantSheetOpen} open={participantSheetOpen}>
        <SheetContent className="overflow-y-auto">
          <SheetHeader>
            <SheetTitle>Participants</SheetTitle>
            <SheetDescription>
              Frozen when the Meeting was created.
            </SheetDescription>
          </SheetHeader>
          <div className="mt-5">
            <MeetingParticipantsPanel
              profiles={profiles}
              snapshot={readySnapshot}
            />
          </div>
        </SheetContent>
      </Sheet>

      <Sheet onOpenChange={setActivitySheetOpen} open={activitySheetOpen}>
        <SheetContent className="flex p-0 sm:max-w-lg">
          <div className="flex min-h-0 flex-1 flex-col">
            <SheetHeader className="border-b px-5 py-4 text-left">
              <SheetTitle>Meeting activity</SheetTitle>
              <SheetDescription>
                Product-level control history from the verified Meeting
                projection.
              </SheetDescription>
            </SheetHeader>
            <MeetingActivityPanel
              activities={activities}
              error={Boolean(activitiesQuery.error)}
              hasOlder={Boolean(activitiesQuery.hasNextPage)}
              isFetching={activitiesQuery.isFetching}
              isFetchingOlder={activitiesQuery.isFetchingNextPage}
              onFetchOlder={() => void activitiesQuery.fetchNextPage()}
              onRetry={() => void activitiesQuery.refetch()}
              profiles={profiles}
            />
          </div>
        </SheetContent>
      </Sheet>

      {meetingAuthority.status !== "current" ? (
        <div
          aria-atomic="true"
          aria-live="polite"
          className="flex shrink-0 items-center gap-3 border-b border-amber-500/30 bg-amber-500/5 px-4 py-2 text-xs"
          data-testid="meeting-authority-banner"
          role="status"
        >
          <AlertTriangle className="size-4 shrink-0 text-amber-600" />
          <span className="min-w-0 flex-1">
            {meetingAuthority.status === "resyncing"
              ? "Rechecking authoritative Meeting state before restoring controls…"
              : "Connection interrupted or the latest read failed. This is the last verified Meeting state; controls remain paused."}
          </span>
          {meetingAuthority.canRetry ? (
            <Button
              data-testid="meeting-authority-retry"
              onClick={() => void meetingAuthority.retry()}
              size="sm"
              variant="outline"
            >
              <RefreshCw className="size-4" />
              Recheck
            </Button>
          ) : null}
        </div>
      ) : null}

      <div
        aria-atomic="true"
        aria-live="polite"
        className="flex shrink-0 items-center gap-2 border-b bg-muted/25 px-4 py-2 text-xs"
        data-testid="meeting-status-strip"
      >
        <span
          className={`size-2 shrink-0 rounded-full ${
            terminal
              ? readySnapshot.lifecycle === "closed"
                ? "bg-emerald-500"
                : "bg-destructive"
              : readySnapshot.lifecycle === "finalizing_actions"
                ? "bg-amber-500"
                : "bg-blue-500"
          }`}
        />
        <span className="min-w-0 flex-1 truncate">{statusText}</span>
      </div>

      {readySnapshot.end ? (
        <div id="meeting-terminal-outcome" tabIndex={-1}>
          <MeetingTerminalSummary
            actionStarted={readySnapshot.action !== null}
            end={readySnapshot.end}
            profiles={profiles}
          />
        </div>
      ) : null}

      <div
        className="flex min-h-0 min-w-0 flex-1 overflow-hidden"
        data-testid="meeting-work-area"
      >
        <div
          className="flex min-h-0 min-w-0 flex-1 flex-col"
          data-testid="meeting-left-workspace"
        >
          <main
            className="min-h-0 min-w-0 flex-1 overflow-y-auto"
            data-testid="meeting-timeline-scroll"
          >
            <MeetingSpeechTimeline
              hasOlder={Boolean(speechesQuery.hasNextPage)}
              isFetchingOlder={speechesQuery.isFetchingNextPage}
              onFetchOlder={() => void speechesQuery.fetchNextPage()}
              profiles={profiles}
              speeches={speeches}
            />
            {speechesQuery.error ? (
              <div className="mx-auto mb-5 max-w-xl rounded-lg border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive">
                Formal Speech could not be verified. Retry the Meeting read.
              </div>
            ) : null}
          </main>
          <MeetingFloorDock
            abortDialogOpen={abortDialogOpen}
            authorityAvailable={meetingAuthority.authorityAvailable}
            boardDraft={boardDraft}
            currentPubkey={identityQuery.data?.pubkey}
            grantRenewalError={
              grantRenewalFailure?.grantId === humanGrantId
                ? grantRenewalFailure.message
                : null
            }
            onAbortDialogOpenChange={setAbortDialogOpen}
            onRefresh={() => void snapshotQuery.refetch()}
            profiles={profiles}
            snapshot={readySnapshot}
          />
        </div>
        {!boardPanelIsOverlay && wideBoardOpen ? (
          <aside
            aria-label="Meeting board panel"
            className="relative flex min-h-0 shrink-0"
            data-testid="meeting-board-wide"
            id="meeting-board-wide-panel"
            style={{ width: boardWidth.widthPx }}
          >
            <button
              aria-label="Resize Meeting board"
              className="group absolute inset-y-0 left-0 z-40 w-3 -translate-x-1/2 cursor-col-resize"
              data-testid="meeting-board-resize-handle"
              onDoubleClick={boardWidth.reset}
              onKeyDown={boardWidth.onResizeKeyDown}
              onPointerDown={boardWidth.onResizeStart}
              title="Drag or use arrow keys to resize. Press Home or double-click to reset."
              type="button"
            >
              <span className="absolute inset-y-0 left-1/2 w-px -translate-x-1/2 bg-border/80 group-focus-visible:bg-ring" />
            </button>
            <MeetingBoardPanel
              board={readySnapshot.board}
              className="h-full w-full border-l"
              editor={
                boardEditable
                  ? {
                      disabled: !meetingAuthority.authorityAvailable,
                      onChange: boardDraft.setValue,
                      value: boardDraft.value,
                    }
                  : undefined
              }
              onDismissStaleDraft={boardDraft.dismissStale}
              staleDraft={boardDraft.stale}
            />
          </aside>
        ) : null}
      </div>
    </div>
  );
}
