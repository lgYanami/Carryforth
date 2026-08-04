import * as React from "react";
import {
  AlertTriangle,
  CheckCircle2,
  ClipboardList,
  ExternalLink,
  RefreshCw,
  ShieldCheck,
  UsersRound,
  XCircle,
} from "lucide-react";

import { useAppShell } from "@/app/AppShellContext";
import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import {
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
  MeetingLifecycle,
  MeetingSnapshot,
  MeetingSpeech,
} from "@/shared/api/tauriMeetings";
import { setVisibleChannel } from "@/shared/api/relayClient";
import { useIdentityQuery } from "@/shared/api/hooks";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { useMediaBreakpoint } from "@/shared/hooks/use-mobile";
import { usePreviewFeatureWarning } from "@/shared/features";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";
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
import { MeetingFloorDock } from "./MeetingFloorDock";
import { MeetingParticipantsPanel } from "./MeetingParticipantsPanel";
import { MeetingSpeechTimeline } from "./MeetingSpeechTimeline";
import { MeetingTerminalSummary } from "./MeetingTerminalSummary";

const EMPTY_PROFILES: Record<string, UserProfileSummary> = {};

function lifecycleLabel(lifecycle: MeetingLifecycle): string {
  switch (lifecycle) {
    case "initializing":
      return "Starting";
    case "active":
      return "In progress";
    case "finalizing_actions":
      return "Recording actions";
    case "closed":
      return "Closed";
    case "aborted":
      return "Aborted";
  }
}

function lifecycleBadgeVariant(
  lifecycle: MeetingLifecycle,
): "secondary" | "info" | "warning" | "success" | "destructive" {
  switch (lifecycle) {
    case "initializing":
      return "secondary";
    case "active":
      return "info";
    case "finalizing_actions":
      return "warning";
    case "closed":
      return "success";
    case "aborted":
      return "destructive";
  }
}

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
  const [boardSheetOpen, setBoardSheetOpen] = React.useState(false);
  React.useEffect(() => {
    if (boardEditable && boardPanelIsOverlay && boardDraft.controlToken) {
      setBoardSheetOpen(true);
    }
  }, [boardDraft.controlToken, boardEditable, boardPanelIsOverlay]);
  const speechesQuery = useMeetingSpeeches({
    meetingId,
    enabled: snapshot !== null,
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

  return (
    <div
      className="relative flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden"
      data-meeting-lifecycle={readySnapshot.lifecycle}
      data-meeting-authority={meetingAuthority.status}
      data-testid="meeting-screen"
    >
      <TopChromeInsetHeader flush>
        <header
          className="flex h-12 items-center gap-2 px-3 sm:px-5"
          data-tauri-drag-region
        >
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-sm font-semibold">
              {readySnapshot.title}
            </h1>
            <p className="hidden truncate text-2xs text-muted-foreground sm:block">
              {readySnapshot.description || "Moderated Meeting"}
            </p>
          </div>
          {sourceChannelId ? (
            <Button
              className="hidden sm:inline-flex"
              onClick={() => void goChannel(sourceChannelId)}
              size="sm"
              variant="ghost"
            >
              <ExternalLink className="size-4" />
              Source
            </Button>
          ) : null}
          <Sheet>
            <SheetTrigger asChild>
              <Button
                data-testid="meeting-participants-trigger"
                size="sm"
                variant="ghost"
              >
                <UsersRound className="size-4" />
                <span className="hidden sm:inline">
                  {readySnapshot.participants.length}
                </span>
              </Button>
            </SheetTrigger>
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
          <Sheet onOpenChange={setBoardSheetOpen} open={boardSheetOpen}>
            <SheetTrigger asChild>
              <Button
                className="xl:hidden"
                data-testid="meeting-board-trigger"
                size="sm"
                variant="outline"
              >
                <ClipboardList className="size-4" />
                Board
              </Button>
            </SheetTrigger>
            <SheetContent className="p-0 sm:max-w-xl">
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
          <Badge variant={lifecycleBadgeVariant(readySnapshot.lifecycle)}>
            {readySnapshot.lifecycle === "closed" ? (
              <CheckCircle2 className="mr-1 size-3" />
            ) : readySnapshot.lifecycle === "aborted" ? (
              <XCircle className="mr-1 size-3" />
            ) : (
              <ShieldCheck className="mr-1 size-3" />
            )}
            {lifecycleLabel(readySnapshot.lifecycle)}
          </Badge>
        </header>
      </TopChromeInsetHeader>

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
        <span className="hidden shrink-0 text-2xs text-muted-foreground md:inline">
          Speech r{readySnapshot.speechRevision} · State r
          {readySnapshot.stateRevision}
        </span>
      </div>

      {readySnapshot.end ? (
        <MeetingTerminalSummary
          actionStarted={readySnapshot.action !== null}
          end={readySnapshot.end}
          profiles={profiles}
        />
      ) : null}

      <div className="flex min-h-0 min-w-0 flex-1">
        <main className="min-w-0 flex-1 overflow-y-auto">
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
        <aside
          aria-label="Meeting board panel"
          className="relative hidden shrink-0 xl:flex"
          data-testid="meeting-board-wide"
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
      </div>
      <MeetingFloorDock
        authorityAvailable={meetingAuthority.authorityAvailable}
        boardDraft={boardDraft}
        currentPubkey={identityQuery.data?.pubkey}
        onRefresh={() => void snapshotQuery.refetch()}
        profiles={profiles}
        snapshot={readySnapshot}
      />
    </div>
  );
}
