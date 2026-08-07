import * as React from "react";
import { AlertTriangle, ClipboardCopy, Hand, Loader2 } from "lucide-react";

import { useMeetingFloorActionMutation } from "@/features/meeting/hooks";
import type { MeetingBoardDraft } from "@/features/meeting/useMeetingBoardDraft";
import { useMeetingActionFinalizationController } from "@/features/meeting/useMeetingActionFinalizationController";
import { useMeetingHostActionController } from "@/features/meeting/useMeetingHostActionController";
import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingFloorAction,
  MeetingFloorActionInput,
  MeetingFloorActionResult,
  MeetingGrantYieldReason,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";
import { copyTextToClipboard } from "@/shared/lib/clipboard";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import { MeetingActionFinalizationCard } from "./MeetingActionFinalizationCard";
import { MeetingOfferControls } from "./MeetingOfferControls";
import { MeetingHostConsole } from "./MeetingHostConsole";
import { MeetingHostAbortDialog } from "./MeetingHostEndControls";
import { MeetingHostObservation } from "./MeetingHostObservation";
import {
  MeetingSpeechComposer,
  type MeetingSpeechDraft,
} from "./MeetingSpeechComposer";

const EMPTY_DRAFT: MeetingSpeechDraft = {
  content: "",
  mentions: [],
  handoffTarget: "",
  handoffType: "question",
  handoffReason: "",
};

type StaleDraft = {
  grantId: string;
  content: string;
};

type MeetingFloorDockProps = {
  abortDialogOpen: boolean;
  authorityAvailable: boolean;
  boardDraft: MeetingBoardDraft;
  currentPubkey?: string;
  grantRenewalError: string | null;
  onAbortDialogOpenChange: (open: boolean) => void;
  onRefresh: () => void;
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
};

function participantName(
  pubkey: string | null,
  profiles: Record<string, UserProfileSummary>,
): string | null {
  if (!pubkey) return null;
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

function ReadOnlyFloor({ message }: { message: string }) {
  return (
    <div
      className="text-center text-xs text-muted-foreground"
      data-testid="meeting-read-only-floor"
    >
      {message}
    </div>
  );
}

export function MeetingFloorDock({
  abortDialogOpen,
  authorityAvailable,
  boardDraft,
  currentPubkey,
  grantRenewalError,
  onAbortDialogOpenChange,
  onRefresh,
  profiles,
  snapshot,
}: MeetingFloorDockProps) {
  const dockRef = React.useRef<HTMLElement>(null);
  const {
    error: floorError,
    isPending,
    mutateAsync,
    reset: resetMutation,
  } = useMeetingFloorActionMutation(snapshot.meetingId);
  const floor = snapshot.floor;
  const normalizedPubkey = currentPubkey?.toLowerCase();
  const participant = snapshot.participants.find(
    (candidate) => candidate.pubkey === normalizedPubkey,
  );
  const ownRequest = floor?.humanQueue.find(
    (request) => request.requesterPubkey === normalizedPubkey,
  );
  const activeOffer = floor?.offer ?? null;
  const activeGrant = floor?.grant ?? null;
  const ownOffer =
    activeOffer?.targetPubkey === normalizedPubkey ? activeOffer : null;
  const ownGrant =
    activeGrant?.holderPubkey === normalizedPubkey ? activeGrant : null;
  const [unresolved, setUnresolved] =
    React.useState<MeetingFloorActionInput | null>(null);
  const [draft, setDraft] = React.useState<MeetingSpeechDraft>(EMPTY_DRAFT);
  const [draftGrantId, setDraftGrantId] = React.useState<string | null>(
    ownGrant?.grantId ?? null,
  );
  const [staleDraft, setStaleDraft] = React.useState<StaleDraft | null>(null);
  const hostController = useMeetingHostActionController({
    onBoardAccepted: boardDraft.markAccepted,
    snapshot,
  });
  const actionController = useMeetingActionFinalizationController(snapshot);

  React.useEffect(() => {
    const currentGrantId = ownGrant?.grantId ?? null;
    if (currentGrantId === draftGrantId) return;
    if (draftGrantId && draft.content.trim()) {
      setStaleDraft({ grantId: draftGrantId, content: draft.content });
    }
    setDraft(EMPTY_DRAFT);
    setDraftGrantId(currentGrantId);
  }, [draft.content, draftGrantId, ownGrant?.grantId]);

  const handleResult = React.useCallback(
    (
      input: MeetingFloorActionInput,
      result: MeetingFloorActionResult,
    ): MeetingFloorActionResult => {
      if (result.status === "indeterminate") {
        setUnresolved(input);
      } else {
        setUnresolved(null);
        if (result.action === "speech") {
          setDraft(EMPTY_DRAFT);
          setStaleDraft((current) =>
            current?.grantId === draftGrantId ? null : current,
          );
        }
      }
      return result;
    },
    [draftGrantId],
  );

  const submit = React.useCallback(
    async (action: MeetingFloorAction) => {
      if (!floor || unresolved || hostController.disabled) return undefined;
      resetMutation();
      const input: MeetingFloorActionInput = {
        submissionId: crypto.randomUUID(),
        meetingId: snapshot.meetingId,
        expectedStateEventId: floor.stateEventId,
        action,
      };
      try {
        const result = await mutateAsync(input);
        return handleResult(input, result);
      } catch {
        // The mutation retains the definitive error for rendering. Native has
        // released this submission, so a later attempt must get a fresh ID.
        return undefined;
      }
    },
    [
      floor,
      handleResult,
      hostController.disabled,
      mutateAsync,
      resetMutation,
      snapshot.meetingId,
      unresolved,
    ],
  );

  const retryExact = React.useCallback(async () => {
    if (!unresolved) return;
    resetMutation();
    try {
      const result = await mutateAsync(unresolved);
      handleResult(unresolved, result);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (
        !message.includes("belongs to a different Community") &&
        !message.includes("belongs to a different identity")
      ) {
        setUnresolved(null);
      }
    }
  }, [handleResult, mutateAsync, resetMutation, unresolved]);

  const disabled =
    !authorityAvailable ||
    isPending ||
    unresolved !== null ||
    hostController.disabled ||
    actionController.disabled;
  const hostControlsDisabled =
    !authorityAvailable ||
    hostController.disabled ||
    actionController.disabled ||
    isPending ||
    unresolved !== null;
  const actionControlsDisabled =
    !authorityAvailable ||
    actionController.disabled ||
    hostController.disabled ||
    isPending ||
    unresolved !== null;
  const renderedHostController = {
    ...hostController,
    disabled: hostControlsDisabled,
  };
  const renderedActionController = {
    ...actionController,
    disabled: actionControlsDisabled,
  };
  const terminal =
    snapshot.lifecycle === "closed" || snapshot.lifecycle === "aborted";
  const moderator = snapshot.participants.find(
    (candidate) => candidate.pubkey === snapshot.moderatorPubkey,
  );
  const observesAgentHost =
    moderator?.participantType === "agent" &&
    participant?.participantType === "human";
  const controlsHumanHost =
    normalizedPubkey === snapshot.moderatorPubkey &&
    participant?.participantType === "human" &&
    !terminal;
  const attentionKey = !authorityAvailable
    ? null
    : ownOffer
      ? `offer:${ownOffer.offerId}`
      : ownGrant
        ? `grant:${ownGrant.grantId}`
        : normalizedPubkey === snapshot.moderatorPubkey && snapshot.action
          ? `action:${snapshot.action.actionRunId}:${snapshot.action.actionWindowEpoch}:${snapshot.action.condition}`
          : normalizedPubkey === snapshot.moderatorPubkey && snapshot.host
            ? `host:${snapshot.host.controlToken}:${snapshot.host.boardControl.phase}`
            : null;
  const previousAttentionRef = React.useRef<string | null>(attentionKey);
  React.useEffect(() => {
    const previous = previousAttentionRef.current;
    previousAttentionRef.current = attentionKey;
    if (!attentionKey || attentionKey === previous) return;
    const activeElement = document.activeElement;
    if (
      activeElement === document.body ||
      (activeElement && dockRef.current?.contains(activeElement))
    ) {
      dockRef.current?.focus({ preventScroll: true });
    }
  }, [attentionKey]);

  return (
    <section
      aria-label="Meeting floor"
      className="min-w-0 shrink-0 border-t bg-muted/20 px-4 py-3"
      data-testid="meeting-floor-dock"
      ref={dockRef}
      tabIndex={-1}
    >
      {unresolved ? (
        <div
          className="mx-auto mb-3 flex max-w-3xl items-start gap-3 rounded-lg border border-amber-500/40 bg-amber-500/5 p-3"
          data-testid="meeting-floor-indeterminate"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <div className="min-w-0 flex-1 text-xs">
            <p className="font-medium">The Relay response was incomplete.</p>
            <p className="mt-1 text-muted-foreground">
              Retry republishes the exact same signed command. Other Floor
              actions remain locked until its receipt is known.
            </p>
          </div>
          <Button
            data-testid="meeting-floor-retry"
            disabled={!authorityAvailable || isPending}
            onClick={() => void retryExact()}
            size="sm"
            variant="outline"
          >
            {isPending ? <Loader2 className="size-4 animate-spin" /> : null}
            Retry exact action
          </Button>
        </div>
      ) : null}

      {hostController.unresolved ? (
        <div
          className="mx-auto mb-3 flex max-w-3xl items-start gap-3 rounded-lg border border-amber-500/40 bg-amber-500/5 p-3"
          data-testid="meeting-host-indeterminate"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <div className="min-w-0 flex-1 text-xs">
            <p className="font-medium">
              The host command receipt is incomplete.
            </p>
            <p className="mt-1 text-muted-foreground">
              Retry republishes the exact same signed event. Board and Floor
              controls remain locked until its outcome is known.
            </p>
          </div>
          <Button
            data-testid="meeting-host-retry"
            disabled={!authorityAvailable || hostController.isPending}
            onClick={() => void hostController.retryExact()}
            size="sm"
            variant="outline"
          >
            {hostController.isPending ? (
              <Loader2 className="size-4 animate-spin" />
            ) : null}
            Retry exact action
          </Button>
        </div>
      ) : null}

      {actionController.unresolved ? (
        <div
          className="mx-auto mb-3 flex max-w-3xl items-start gap-3 rounded-lg border border-amber-500/40 bg-amber-500/5 p-3"
          data-testid="meeting-action-indeterminate"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <div className="min-w-0 flex-1 text-xs">
            <p className="font-medium">
              The action-finalization receipt is incomplete.
            </p>
            <p className="mt-1 text-muted-foreground">
              Retry republishes the exact same signed event. Meeting close and
              recovery controls remain locked until its outcome is known.
            </p>
          </div>
          <Button
            data-testid="meeting-action-retry-exact"
            disabled={!authorityAvailable || actionController.isPending}
            onClick={() => void actionController.retryExact()}
            size="sm"
            variant="outline"
          >
            {actionController.isPending ? (
              <Loader2 className="size-4 animate-spin" />
            ) : null}
            Retry exact action
          </Button>
        </div>
      ) : null}

      {floorError ? (
        <div
          className="mx-auto mb-3 max-w-3xl rounded-lg border border-destructive/35 bg-destructive/5 px-3 py-2 text-xs text-destructive"
          data-testid="meeting-floor-error"
        >
          {floorError instanceof Error
            ? floorError.message
            : "The Floor action was rejected."}
        </div>
      ) : null}

      {ownGrant && grantRenewalError ? (
        <div
          className="mx-auto mb-3 max-w-3xl rounded-lg border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-xs text-amber-700 dark:text-amber-300"
          data-testid="meeting-grant-renewal-error"
        >
          Buzz could not retain this speaking window. Your draft is preserved
          while the authoritative Meeting state is checked.
        </div>
      ) : null}

      {hostController.error ? (
        <div
          className="mx-auto mb-3 max-w-3xl rounded-lg border border-destructive/35 bg-destructive/5 px-3 py-2 text-xs text-destructive"
          data-testid="meeting-host-error"
        >
          {hostController.error.message}
        </div>
      ) : null}

      {actionController.error ? (
        <div
          className="mx-auto mb-3 max-w-3xl rounded-lg border border-destructive/35 bg-destructive/5 px-3 py-2 text-xs text-destructive"
          data-testid="meeting-action-error"
        >
          {actionController.error.message}
        </div>
      ) : null}

      {staleDraft ? (
        <div
          className="mx-auto mb-3 flex max-w-3xl items-center gap-3 rounded-lg border border-dashed px-3 py-2 text-xs"
          data-testid="meeting-stale-speech-draft"
        >
          <span className="min-w-0 flex-1 text-muted-foreground">
            A draft from an expired or completed Grant was preserved. It will
            not be reused for another Grant.
          </span>
          <Button
            onClick={() =>
              copyTextToClipboard(staleDraft.content, "Speech draft copied")
            }
            size="sm"
            variant="ghost"
          >
            <ClipboardCopy className="size-4" />
            Copy draft
          </Button>
          <Button
            aria-label="Dismiss stale Speech draft"
            onClick={() => setStaleDraft(null)}
            size="sm"
            variant="ghost"
          >
            Dismiss
          </Button>
        </div>
      ) : null}

      {observesAgentHost ? (
        <MeetingHostObservation
          authorityAvailable={authorityAvailable}
          onRefresh={onRefresh}
          profiles={profiles}
          snapshot={snapshot}
        />
      ) : null}

      {!authorityAvailable ? (
        <ReadOnlyFloor message="Meeting controls are paused until Desktop verifies the latest authoritative state." />
      ) : terminal ? (
        <ReadOnlyFloor message="This Meeting is read-only. Its final Board and formal Speech remain available." />
      ) : snapshot.lifecycle === "finalizing_actions" ? (
        normalizedPubkey === snapshot.moderatorPubkey &&
        participant?.participantType === "human" ? (
          <MeetingActionFinalizationCard
            actionController={renderedActionController}
            hostController={renderedHostController}
            onRefresh={onRefresh}
            profiles={profiles}
            snapshot={snapshot}
          />
        ) : (
          <ReadOnlyFloor
            message={
              snapshot.participants.find(
                (candidate) => candidate.pubkey === snapshot.moderatorPubkey,
              )?.participantType === "agent"
                ? "The host Agent is recording actions from the frozen final Board. The discussion Floor remains read-only."
                : "The Human host is recording actions from the frozen final Board. Only the frozen host can submit recovery or completion decisions."
            }
          />
        )
      ) : !floor ? (
        <ReadOnlyFloor message="The Relay is preparing the authoritative Meeting Floor." />
      ) : participant?.participantType !== "human" ? (
        <ReadOnlyFloor message="You are observing the authoritative Meeting. Human Floor controls are not available for this identity." />
      ) : ownOffer ? (
        <MeetingOfferControls
          key={ownOffer.offerId}
          disabled={disabled}
          offer={ownOffer}
          onDeadline={onRefresh}
          onSubmit={async (action) => {
            await submit(action);
          }}
        />
      ) : ownGrant ? (
        <MeetingSpeechComposer
          key={ownGrant.grantId}
          disabled={disabled}
          draft={draft}
          grant={ownGrant}
          onChange={setDraft}
          onDeadline={onRefresh}
          onSubmit={async () => {
            await submit({
              type: "speech",
              content: draft.content,
              mentions: draft.mentions,
              handoff: draft.handoffTarget
                ? {
                    targetPubkey: draft.handoffTarget,
                    handoffType: draft.handoffType,
                    reason: draft.handoffReason,
                  }
                : undefined,
            });
          }}
          onYield={async (reasonCode: MeetingGrantYieldReason) => {
            if (draft.content.trim()) {
              setStaleDraft({
                grantId: ownGrant.grantId,
                content: draft.content,
              });
            }
            await submit({ type: "grant_yield", reasonCode });
          }}
          participants={snapshot.participants}
          profiles={profiles}
          selfPubkey={normalizedPubkey ?? ""}
        />
      ) : ownRequest ? (
        <div className="mx-auto flex max-w-3xl items-center gap-3">
          <Hand className="size-4 text-blue-500" />
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium">You requested the floor</p>
            <p className="text-xs text-muted-foreground">
              Queue position {ownRequest.queuePosition}. The current speaker is
              not interrupted.
            </p>
          </div>
          <Button
            data-testid="meeting-floor-withdraw"
            disabled={disabled}
            onClick={() => void submit({ type: "withdraw" })}
            size="sm"
            variant="outline"
          >
            Withdraw request
          </Button>
        </div>
      ) : normalizedPubkey === snapshot.moderatorPubkey ? (
        <MeetingHostConsole
          actionController={renderedActionController}
          boardDraft={boardDraft}
          controller={renderedHostController}
          currentPubkey={normalizedPubkey}
          onRefresh={onRefresh}
          profiles={profiles}
          snapshot={snapshot}
        />
      ) : (
        <div className="mx-auto flex max-w-3xl items-center gap-3">
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium">
              {snapshot.currentSpeakerPubkey
                ? `${participantName(snapshot.currentSpeakerPubkey, profiles)} has the floor`
                : snapshot.currentOfferPubkey
                  ? `Waiting for ${participantName(snapshot.currentOfferPubkey, profiles)}`
                  : "The host controls the floor"}
            </p>
            <p className="text-xs text-muted-foreground">
              Requesting does not interrupt an active Grant.
            </p>
          </div>
          <Button
            data-testid="meeting-floor-request"
            disabled={disabled}
            onClick={() => void submit({ type: "request" })}
            size="sm"
          >
            {isPending ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Hand className="size-4" />
            )}
            Request floor
          </Button>
        </div>
      )}
      {controlsHumanHost ? (
        <MeetingHostAbortDialog
          actionPhase={snapshot.lifecycle === "finalizing_actions"}
          disabled={hostControlsDisabled}
          onOpenChange={onAbortDialogOpenChange}
          open={abortDialogOpen}
          submit={renderedHostController.submit}
        />
      ) : null}
    </section>
  );
}
