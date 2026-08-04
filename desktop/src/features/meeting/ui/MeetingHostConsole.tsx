import {
  AlertTriangle,
  Clock3,
  Hand,
  Loader2,
  RotateCcw,
  Save,
  ShieldCheck,
  UsersRound,
} from "lucide-react";

import type { MeetingBoardDraft } from "@/features/meeting/useMeetingBoardDraft";
import type { MeetingActionFinalizationController } from "@/features/meeting/useMeetingActionFinalizationController";
import type { MeetingHostActionController } from "@/features/meeting/useMeetingHostActionController";
import {
  meetingDeadlineLabel,
  useMeetingDeadline,
} from "@/features/meeting/useMeetingDeadline";
import type { UserProfileSummary } from "@/shared/api/types";
import type { MeetingSnapshot } from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { MeetingHostEndControls } from "./MeetingHostEndControls";
import { MeetingHostHandoffList } from "./MeetingHostHandoffList";
import { MeetingHostIntentList } from "./MeetingHostIntentList";

const MAX_BOARD_BYTES = 65_536;

function participantName(
  pubkey: string | null,
  profiles: Record<string, UserProfileSummary>,
): string {
  if (!pubkey) return "a participant";
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

export function MeetingHostConsole({
  actionController,
  boardDraft,
  controller,
  currentPubkey,
  onRefresh,
  profiles,
  snapshot,
}: {
  actionController: MeetingActionFinalizationController;
  boardDraft: MeetingBoardDraft;
  controller: MeetingHostActionController;
  currentPubkey: string;
  onRefresh: () => void;
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
}) {
  const host = snapshot.host;
  const boardPending = host?.boardControl.phase === "board_pending";
  const deadlineMs = boardPending
    ? (host?.boardControl.boardDeadlineAtMs ?? null)
    : host?.canSelect
      ? host.decisionDeadlineMs
      : null;
  const remainingMs = useMeetingDeadline(deadlineMs, onRefresh);
  if (!host) {
    return (
      <p className="text-center text-xs text-muted-foreground">
        The Relay is preparing the verified host control projection.
      </p>
    );
  }

  const deadlineExpired = remainingMs !== null && remainingMs <= 0;
  const boardTokenCurrent =
    boardDraft.controlToken !== null &&
    boardDraft.controlToken === host.controlToken;
  const boardBytes = new TextEncoder().encode(boardDraft.value).length;
  const boardValid =
    boardTokenCurrent &&
    Boolean(boardDraft.value.trim()) &&
    boardBytes <= MAX_BOARD_BYTES &&
    !boardDraft.value.includes("\0");
  const selfIntent = host.pendingIntents.find(
    (intent) => intent.authorPubkey === currentPubkey,
  );
  const humanRequest = snapshot.floor?.humanQueue[0] ?? null;
  const otherOffer = snapshot.floor?.offer ?? null;
  const otherGrant = snapshot.floor?.grant ?? null;
  const selectionEnabled = host.canSelect && !deadlineExpired;
  const noDecisionWork =
    host.pendingIntents.length === 0 && host.openHandoffs.length === 0;
  const canFinalizeActions =
    snapshot.policy === "moderated-board-actions-v2" &&
    snapshot.phase === "moderator_idle" &&
    host.canClose &&
    noDecisionWork &&
    !deadlineExpired;

  return (
    <div
      className="mx-auto max-h-[45vh] w-full max-w-4xl overflow-y-auto rounded-xl border bg-background p-4 shadow-xs"
      data-board-phase={host.boardControl.phase}
      data-testid="meeting-host-console"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-3">
          <div className="rounded-full bg-primary/10 p-2 text-primary">
            <ShieldCheck className="size-5" />
          </div>
          <div>
            <h2 className="text-sm font-semibold">
              {boardPending ? "Review the Meeting Board" : "Host controls"}
            </h2>
            <p className="mt-0.5 text-xs text-muted-foreground">
              {boardPending
                ? "Board Maintenance must finish before choosing the next speaker."
                : "Arrange the next authoritative Floor transition."}
            </p>
          </div>
        </div>
        {deadlineMs !== null ? (
          <Badge variant={deadlineExpired ? "warning" : "outline"}>
            <Clock3 className="mr-1 size-3" />
            {boardPending ? "Board" : "Floor"} ·{" "}
            {meetingDeadlineLabel(remainingMs)}
          </Badge>
        ) : null}
      </div>

      {boardPending ? (
        <div className="mt-4 rounded-lg border border-amber-500/35 bg-amber-500/5 p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <p className="text-sm font-medium">Board window</p>
              <p className="text-xs text-muted-foreground">
                The editor is bound only to Board window{" "}
                {host.boardControl.boardWindow}. Floor time has not started.
              </p>
            </div>
            <span
              className={`text-xs ${
                boardBytes > MAX_BOARD_BYTES
                  ? "text-destructive"
                  : "text-muted-foreground"
              }`}
            >
              {boardBytes.toLocaleString()} / {MAX_BOARD_BYTES.toLocaleString()}{" "}
              bytes
            </span>
          </div>
          {deadlineExpired || !boardTokenCurrent ? (
            <div className="mt-3 flex items-start gap-2 rounded-md border border-amber-500/30 px-3 py-2 text-xs">
              <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
              <span>
                This Board window can no longer accept a submission. Refreshing
                the authoritative state; edited text will remain copyable.
              </span>
            </div>
          ) : null}
          <div className="mt-3 flex flex-wrap justify-end gap-2">
            <Button
              data-testid="meeting-board-unchanged"
              disabled={
                controller.disabled || deadlineExpired || !boardTokenCurrent
              }
              onClick={() =>
                void controller.submit({ type: "board_unchanged" })
              }
              size="sm"
              variant="outline"
            >
              Board unchanged
            </Button>
            <Button
              data-testid="meeting-board-save"
              disabled={
                controller.disabled ||
                deadlineExpired ||
                !boardValid ||
                boardDraft.value === boardDraft.initialValue
              }
              onClick={() =>
                void controller.submit({
                  type: "board_update",
                  body: boardDraft.value,
                })
              }
              size="sm"
            >
              {controller.isPending ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <Save className="size-4" />
              )}
              Save and continue
            </Button>
          </div>
        </div>
      ) : null}

      {humanRequest ? (
        <div
          className="mt-4 flex items-start gap-3 rounded-lg border border-blue-500/35 bg-blue-500/5 p-3"
          data-testid="meeting-host-human-priority"
        >
          <Hand className="mt-0.5 size-4 shrink-0 text-blue-600" />
          <div>
            <p className="text-sm font-medium">
              Human floor request has priority
            </p>
            <p className="text-xs text-muted-foreground">
              {participantName(humanRequest.requesterPubkey, profiles)} is queue
              position {humanRequest.queuePosition}. The host cannot reject or
              reorder this request.
            </p>
          </div>
        </div>
      ) : null}

      {!boardPending && otherOffer ? (
        <div className="mt-4 flex items-start gap-3 rounded-lg border p-3">
          <UsersRound className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium">
              Waiting for {participantName(otherOffer.targetPubkey, profiles)}
            </p>
            <p className="text-xs text-muted-foreground">
              An Offer was created from{" "}
              {otherOffer.allocationSource.replaceAll("_", " ")}; a Grant does
              not exist until it is accepted.
            </p>
          </div>
        </div>
      ) : null}

      {!boardPending && otherGrant ? (
        <div className="mt-4 flex items-start gap-3 rounded-lg border p-3">
          <UsersRound className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium">
              {participantName(otherGrant.holderPubkey, profiles)} has the floor
            </p>
            <p className="text-xs text-muted-foreground">
              Recall only returns control after the current Grant completes; it
              never interrupts a formal Speech.
            </p>
          </div>
        </div>
      ) : null}

      {host.canRecall && !humanRequest ? (
        <div className="mt-3 flex justify-end">
          <Button
            data-testid="meeting-host-recall"
            disabled={controller.disabled}
            onClick={() => void controller.submit({ type: "recall" })}
            size="sm"
            variant="outline"
          >
            <RotateCcw className="size-4" />
            Return control after this turn
          </Button>
        </div>
      ) : null}

      {host.boardControl.boardOutcome === "timed_out" && !boardPending ? (
        <div
          className="mt-4 flex items-start gap-2 rounded-lg border border-amber-500/35 bg-amber-500/5 p-3 text-xs"
          data-testid="meeting-board-timeout-notice"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <span>
            Board Maintenance timed out. This Floor Decision has its own full
            deadline, but normal Close remains unavailable until a later Board
            window is explicitly completed.
          </span>
        </div>
      ) : null}

      <div className="mt-4 space-y-4">
        <MeetingHostIntentList
          currentPubkey={currentPubkey}
          disabled={controller.disabled}
          profiles={profiles}
          selectionEnabled={!boardPending && selectionEnabled}
          snapshot={snapshot}
          submit={controller.submit}
        />
        {!boardPending ? (
          <MeetingHostHandoffList
            disabled={controller.disabled}
            hasSelfIntent={Boolean(selfIntent)}
            profiles={profiles}
            selectionEnabled={selectionEnabled}
            snapshot={snapshot}
            submit={controller.submit}
          />
        ) : null}
      </div>

      {!boardPending && host.canSelect && noDecisionWork ? (
        <div
          className="mt-4 rounded-lg border border-dashed px-3 py-4 text-center"
          data-testid="meeting-host-idle"
        >
          <p className="text-sm font-medium">Waiting for a speaking intent</p>
          <p className="mt-1 text-xs text-muted-foreground">
            No empty Speech or polling action is created. New authoritative work
            will reopen the host flow.
          </p>
        </div>
      ) : null}

      {!boardPending ? (
        <MeetingHostEndControls
          canClose={host.canClose && !deadlineExpired}
          canFinalizeActions={canFinalizeActions}
          disabled={controller.disabled}
          submit={controller.submit}
          submitFinalization={actionController.submit}
        />
      ) : null}
    </div>
  );
}
