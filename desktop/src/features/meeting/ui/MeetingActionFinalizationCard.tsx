import * as React from "react";
import {
  AlertTriangle,
  ArrowLeft,
  CheckCircle2,
  Clock3,
  ExternalLink,
  Loader2,
  RefreshCw,
} from "lucide-react";

import { useAppNavigation } from "@/app/navigation/useAppNavigation";
import type { MeetingActionFinalizationController } from "@/features/meeting/useMeetingActionFinalizationController";
import {
  meetingDeadlineLabel,
  useMeetingDeadline,
} from "@/features/meeting/useMeetingDeadline";
import type { MeetingHostActionController } from "@/features/meeting/useMeetingHostActionController";
import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingActionBlockReason,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui/alert-dialog";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Textarea } from "@/shared/ui/textarea";

const BLOCK_LABELS: Record<MeetingActionBlockReason, string> = {
  external_operation_failed: "An external operation failed",
  external_state_conflict: "External state conflicts with the Board decision",
  tool_unavailable: "A required tool is unavailable",
  provider_failure: "An external provider is unavailable",
  action_deadline_exceeded: "The action window expired",
};

const BLOCKED_STATUS_LABELS: Record<string, string> = {
  ...BLOCK_LABELS,
  affinity_lost: "The original execution context was lost",
  action_lease_expired: "The action host stopped renewing its lease",
  action_operator_deadline_exceeded: "The operator safety limit was reached",
};

const PROGRESS_LABELS = {
  reasoning: "Reasoning",
  tool_call: "Calling a tool",
  tool_result: "Processing a tool result",
  finalizing: "Preparing completion",
  waiting_human: "Waiting for the Human host",
} as const;

function hostName(
  pubkey: string,
  profiles: Record<string, UserProfileSummary>,
): string {
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

export function MeetingActionFinalizationCard({
  actionController,
  hostController,
  onRefresh,
  profiles,
  renewalError,
  snapshot,
}: {
  actionController: MeetingActionFinalizationController;
  hostController: MeetingHostActionController;
  onRefresh: () => void;
  profiles: Record<string, UserProfileSummary>;
  renewalError: string | null;
  snapshot: MeetingSnapshot;
}) {
  const { goChannel, goView } = useAppNavigation();
  const action = snapshot.action;
  const [blockOpen, setBlockOpen] = React.useState(false);
  const [returnOpen, setReturnOpen] = React.useState(false);
  const [confirmOpen, setConfirmOpen] = React.useState(false);
  const [blockReasonCode, setBlockReasonCode] =
    React.useState<MeetingActionBlockReason>("external_operation_failed");
  const [blockReason, setBlockReason] = React.useState("");
  const remainingMs = useMeetingDeadline(
    action?.condition === "runnable"
      ? (action.actionDeadlineAtMs ?? null)
      : null,
    onRefresh,
  );

  if (!action) {
    return (
      <p className="text-center text-xs text-muted-foreground">
        The Relay is preparing the verified action-finalization state.
      </p>
    );
  }

  const runnable = action.condition === "runnable";
  const blocked = action.condition === "blocked";
  const deadlineExpired = runnable && remainingMs !== null && remainingMs <= 0;
  const disabled =
    actionController.disabled || hostController.disabled || deadlineExpired;

  return (
    <section
      aria-label="Meeting action finalization"
      className="mx-auto max-h-[45vh] w-full max-w-4xl overflow-y-auto rounded-xl border bg-background p-4 shadow-xs"
      data-action-condition={action.condition}
      data-testid="meeting-action-finalization-card"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-semibold">
              {blocked
                ? "Action recording is blocked"
                : "Record meeting actions"}
            </h2>
            <Badge variant={blocked ? "warning" : "info"}>
              {blocked ? "Blocked" : "In progress"}
            </Badge>
          </div>
          <p className="mt-1 text-xs text-muted-foreground">
            {hostName(snapshot.moderatorPubkey, profiles)} is responsible for
            recording the final Board outcome. The Meeting remains open and its
            discussion Floor is frozen.
          </p>
        </div>
        {runnable && action.actionDeadlineAtMs !== null ? (
          <Badge variant={deadlineExpired ? "warning" : "outline"}>
            <Clock3 className="mr-1 size-3" />
            Lease · {meetingDeadlineLabel(remainingMs)}
          </Badge>
        ) : null}
      </div>

      {snapshot.policy === "moderated-board-actions-v3" ? (
        <div
          className="mt-3 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground"
          data-testid="meeting-action-progress"
        >
          <span>
            Stage:{" "}
            {action.lastProgressStage
              ? PROGRESS_LABELS[action.lastProgressStage]
              : "Starting"}
          </span>
          <span>Renewals: {action.progressSeq}</span>
          {action.lastProgressAtMs !== null ? (
            <span>
              Last progress:{" "}
              {new Date(action.lastProgressAtMs).toLocaleTimeString()}
            </span>
          ) : null}
        </div>
      ) : null}

      {runnable && renewalError ? (
        <div
          className="mt-4 flex items-start gap-2 rounded-lg border border-amber-500/35 bg-amber-500/5 p-3 text-xs"
          data-testid="meeting-action-renewal-error"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <span>
            Buzz could not retain this action-recording window. The canonical
            Meeting state is being rechecked and renewal will retry while this
            exact window remains current.
          </span>
        </div>
      ) : null}

      <div className="mt-4 rounded-lg border bg-muted/20 p-3">
        <p className="text-sm font-medium">Final Board is frozen</p>
        <p className="mt-1 text-xs text-muted-foreground">
          Use the Board panel as the meeting record. Meeting does not inspect,
          count, or restrict operations performed in Project View or another
          system.
        </p>
        <div className="mt-3 flex flex-wrap gap-2">
          <Button
            data-testid="meeting-action-open-view"
            onClick={() => void goView()}
            size="sm"
            variant="outline"
          >
            <ExternalLink className="size-4" />
            Open Project View
          </Button>
          {snapshot.sourceChannelId ? (
            <Button
              data-testid="meeting-action-open-source"
              onClick={() => void goChannel(snapshot.sourceChannelId ?? "")}
              size="sm"
              variant="ghost"
            >
              <ExternalLink className="size-4" />
              Open source Channel
            </Button>
          ) : null}
        </div>
      </div>

      {blocked ? (
        <div
          className="mt-4 flex items-start gap-3 rounded-lg border border-amber-500/35 bg-amber-500/5 p-3"
          data-testid="meeting-action-blocked"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <div className="min-w-0 flex-1">
            <p className="text-sm font-medium">
              {action.lastErrorCode &&
              action.lastErrorCode in BLOCKED_STATUS_LABELS
                ? BLOCKED_STATUS_LABELS[action.lastErrorCode]
                : "The current action window cannot continue"}
            </p>
            <p className="mt-1 text-xs text-muted-foreground">
              External effects are retained. Retry opens a new authoritative
              window; check the target system before doing more work.
            </p>
          </div>
        </div>
      ) : null}

      {deadlineExpired ? (
        <div
          className="mt-4 flex items-start gap-2 rounded-lg border border-amber-500/35 bg-amber-500/5 p-3 text-xs"
          data-testid="meeting-action-deadline-expired"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0 text-amber-600" />
          <span>
            This action window can no longer accept completion. Refreshing the
            authoritative Meeting state before offering recovery controls.
          </span>
        </div>
      ) : null}

      <div className="mt-4 flex flex-wrap items-center justify-end gap-2 border-t pt-3">
        <div className="flex flex-wrap justify-end gap-2">
          <Button
            data-testid="meeting-action-return-board"
            disabled={actionController.disabled || hostController.disabled}
            onClick={() => setReturnOpen(true)}
            size="sm"
            variant="ghost"
          >
            <ArrowLeft className="size-4" />
            Return to Board
          </Button>
          {blocked ? (
            <Button
              data-testid="meeting-action-retry"
              disabled={actionController.disabled || hostController.disabled}
              onClick={() => void actionController.submit({ type: "retry" })}
              size="sm"
            >
              {actionController.isPending ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <RefreshCw className="size-4" />
              )}
              Retry action window
            </Button>
          ) : null}
          {runnable ? (
            <>
              <Button
                data-testid="meeting-action-block"
                disabled={disabled}
                onClick={() => setBlockOpen(true)}
                size="sm"
                variant="outline"
              >
                Temporarily unable…
              </Button>
              <Button
                data-testid="meeting-action-confirm"
                disabled={disabled}
                onClick={() => setConfirmOpen(true)}
                size="sm"
              >
                <CheckCircle2 className="size-4" />
                Confirm actions and close…
              </Button>
            </>
          ) : null}
        </div>
      </div>

      <AlertDialog onOpenChange={setBlockOpen} open={blockOpen}>
        <AlertDialogContent data-testid="meeting-action-block-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>
              Report action recording blocked?
            </AlertDialogTitle>
            <AlertDialogDescription>
              This ends the current runnable window without rolling back any
              external effects. A later retry creates a new window.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="space-y-3">
            <select
              aria-label="Action block category"
              className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
              disabled={actionController.disabled}
              onChange={(event) =>
                setBlockReasonCode(
                  event.target.value as MeetingActionBlockReason,
                )
              }
              value={blockReasonCode}
            >
              {Object.entries(BLOCK_LABELS).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
            <Textarea
              aria-label="Action block explanation"
              disabled={actionController.disabled}
              maxLength={1024}
              onChange={(event) => setBlockReason(event.target.value)}
              placeholder="Optional explanation for participants"
              rows={3}
              value={blockReason}
            />
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel asChild>
              <Button type="button" variant="outline">
                Cancel
              </Button>
            </AlertDialogCancel>
            <AlertDialogAction
              data-testid="meeting-action-block-confirm"
              disabled={disabled}
              onClick={async (event) => {
                event.preventDefault();
                const result = await actionController.submit({
                  type: "block",
                  reasonCode: blockReasonCode,
                  reason: blockReason.trim() || undefined,
                });
                if (result?.status === "accepted") setBlockOpen(false);
              }}
            >
              Mark blocked
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog onOpenChange={setReturnOpen} open={returnOpen}>
        <AlertDialogContent data-testid="meeting-action-return-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>Return to the Meeting Board?</AlertDialogTitle>
            <AlertDialogDescription>
              The current action run will end and a new Board Maintenance window
              will open. Any Project View or other external effects that already
              occurred will remain in place.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel asChild>
              <Button type="button" variant="outline">
                Stay in action recording
              </Button>
            </AlertDialogCancel>
            <AlertDialogAction
              data-testid="meeting-action-return-confirm"
              disabled={actionController.disabled || hostController.disabled}
              onClick={async (event) => {
                event.preventDefault();
                const result = await actionController.submit({
                  type: "return_to_board",
                });
                if (result?.status === "accepted") setReturnOpen(false);
              }}
            >
              Keep external effects and return
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog onOpenChange={setConfirmOpen} open={confirmOpen}>
        <AlertDialogContent data-testid="meeting-action-confirm-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>Confirm actions and close?</AlertDialogTitle>
            <AlertDialogDescription>
              I confirm that the actions from the final Meeting Board that
              needed recording before normal close have been recorded, or that
              no new recording is required. This does not mean the resulting
              Work has been completed.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel asChild>
              <Button type="button" variant="outline">
                Continue recording
              </Button>
            </AlertDialogCancel>
            <AlertDialogAction
              data-testid="meeting-action-confirm-submit"
              disabled={disabled}
              onClick={async (event) => {
                event.preventDefault();
                const result = await actionController.submit({
                  type: "confirm",
                });
                if (result?.status === "accepted") setConfirmOpen(false);
              }}
            >
              Confirm and close meeting
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
