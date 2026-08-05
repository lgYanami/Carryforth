import * as React from "react";
import { CheckCircle2, ClipboardCheck } from "lucide-react";

import type {
  MeetingAbortReason,
  MeetingActionFinalizationAction,
  MeetingActionFinalizationResult,
  MeetingHostAction,
  MeetingHostActionResult,
} from "@/shared/api/tauriMeetings";
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
import { Button, buttonVariants } from "@/shared/ui/button";
import { Textarea } from "@/shared/ui/textarea";

type SubmitHostAction = (
  action: MeetingHostAction,
) => Promise<MeetingHostActionResult | undefined>;

type SubmitFinalizationAction = (
  action: MeetingActionFinalizationAction,
) => Promise<MeetingActionFinalizationResult | undefined>;

const ABORT_LABELS: Record<MeetingAbortReason, string> = {
  goal_unreachable: "The discussion goal is unreachable",
  insufficient_information: "Required information is unavailable",
  discussion_blocked: "The discussion is blocked",
  unable_to_form_conclusion: "No valid conclusion can be formed",
  moderator_unable_to_continue: "The host cannot continue",
};

export function MeetingHostAbortDialog({
  actionPhase = false,
  disabled,
  onOpenChange,
  open,
  submit,
}: {
  actionPhase?: boolean;
  disabled: boolean;
  onOpenChange: (open: boolean) => void;
  open: boolean;
  submit: SubmitHostAction;
}) {
  const [reasonCode, setReasonCode] =
    React.useState<MeetingAbortReason>("goal_unreachable");
  const [reason, setReason] = React.useState("");

  return (
    <AlertDialog onOpenChange={onOpenChange} open={open}>
      <AlertDialogContent
        data-testid={
          actionPhase
            ? "meeting-action-abort-dialog"
            : "meeting-host-abort-dialog"
        }
      >
        <AlertDialogHeader>
          <AlertDialogTitle>Abort this meeting?</AlertDialogTitle>
          <AlertDialogDescription>
            Abort is a terminal outcome for a meeting that cannot successfully
            conclude. It does not roll back external effects
            {actionPhase ? " that may already have occurred" : ""}.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="space-y-3">
          <select
            aria-label="Meeting abort category"
            className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
            disabled={disabled}
            onChange={(event) =>
              setReasonCode(event.target.value as MeetingAbortReason)
            }
            value={reasonCode}
          >
            {Object.entries(ABORT_LABELS).map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
          <Textarea
            aria-label="Meeting abort explanation"
            disabled={disabled}
            maxLength={1024}
            onChange={(event) => setReason(event.target.value)}
            placeholder="Optional explanation for participants"
            rows={3}
            value={reason}
          />
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel asChild>
            <Button type="button" variant="outline">
              Cancel
            </Button>
          </AlertDialogCancel>
          <AlertDialogAction
            className={buttonVariants({ variant: "destructive" })}
            data-testid={
              actionPhase
                ? "meeting-action-abort-confirm"
                : "meeting-host-abort-confirm"
            }
            disabled={disabled}
            onClick={async (event) => {
              event.preventDefault();
              const result = await submit({
                type: "abort",
                reasonCode,
                reason: reason.trim() || undefined,
              });
              if (result?.status === "accepted") onOpenChange(false);
            }}
          >
            Abort permanently
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

export function MeetingHostEndControls({
  canClose,
  canFinalizeActions,
  disabled,
  submit,
  submitFinalization,
}: {
  canClose: boolean;
  canFinalizeActions: boolean;
  disabled: boolean;
  submit: SubmitHostAction;
  submitFinalization: SubmitFinalizationAction;
}) {
  const [closeOpen, setCloseOpen] = React.useState(false);
  const [actionsOpen, setActionsOpen] = React.useState(false);

  return (
    <div
      className="flex flex-wrap items-center justify-end gap-2 border-t pt-3"
      data-testid="meeting-host-end-controls"
    >
      <div className="flex flex-wrap justify-end gap-2">
        {canFinalizeActions ? (
          <Button
            data-testid="meeting-host-begin-actions"
            disabled={disabled}
            onClick={() => setActionsOpen(true)}
            size="sm"
            variant="outline"
          >
            <ClipboardCheck className="size-4" />
            Record actions, then close…
          </Button>
        ) : null}
        {canClose ? (
          <Button
            data-testid="meeting-host-close"
            disabled={disabled}
            onClick={() => setCloseOpen(true)}
            size="sm"
          >
            <CheckCircle2 className="size-4" />
            Close meeting
          </Button>
        ) : null}
      </div>

      <AlertDialog onOpenChange={setActionsOpen} open={actionsOpen}>
        <AlertDialogContent data-testid="meeting-host-begin-actions-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>Record actions before closing?</AlertDialogTitle>
            <AlertDialogDescription>
              The current final Board will be frozen and the discussion Floor
              will close. You can use the existing Project View or another
              system, then return here to confirm the outcome. Meeting does not
              create a Plan or Step.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel asChild>
              <Button type="button" variant="outline">
                Keep discussing
              </Button>
            </AlertDialogCancel>
            <AlertDialogAction
              data-testid="meeting-host-begin-actions-confirm"
              disabled={disabled}
              onClick={async (event) => {
                event.preventDefault();
                const result = await submitFinalization({ type: "begin" });
                if (result?.status === "accepted") setActionsOpen(false);
              }}
            >
              Freeze Board and continue
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog onOpenChange={setCloseOpen} open={closeOpen}>
        <AlertDialogContent data-testid="meeting-host-close-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>Close this meeting?</AlertDialogTitle>
            <AlertDialogDescription>
              Confirm that the discussion goal has been reached, a valid
              conclusion is recorded on the final Board, and no action
              finalization is needed.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel asChild>
              <Button type="button" variant="outline">
                Keep discussing
              </Button>
            </AlertDialogCancel>
            <AlertDialogAction
              data-testid="meeting-host-close-confirm"
              disabled={disabled}
              onClick={async (event) => {
                event.preventDefault();
                const result = await submit({ type: "close" });
                if (result?.status === "accepted") setCloseOpen(false);
              }}
            >
              Confirm goal and close
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
