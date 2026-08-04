import * as React from "react";
import { CheckCircle2, OctagonX } from "lucide-react";

import type {
  MeetingAbortReason,
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

const ABORT_LABELS: Record<MeetingAbortReason, string> = {
  goal_unreachable: "The discussion goal is unreachable",
  insufficient_information: "Required information is unavailable",
  discussion_blocked: "The discussion is blocked",
  unable_to_form_conclusion: "No valid conclusion can be formed",
  moderator_unable_to_continue: "The host cannot continue",
};

export function MeetingHostEndControls({
  canClose,
  disabled,
  submit,
}: {
  canClose: boolean;
  disabled: boolean;
  submit: SubmitHostAction;
}) {
  const [closeOpen, setCloseOpen] = React.useState(false);
  const [abortOpen, setAbortOpen] = React.useState(false);
  const [abortReasonCode, setAbortReasonCode] =
    React.useState<MeetingAbortReason>("goal_unreachable");
  const [abortReason, setAbortReason] = React.useState("");

  return (
    <div
      className="flex flex-wrap items-center justify-between gap-2 border-t pt-3"
      data-testid="meeting-host-end-controls"
    >
      <Button
        data-testid="meeting-host-abort"
        disabled={disabled}
        onClick={() => setAbortOpen(true)}
        size="sm"
        variant="ghost"
      >
        <OctagonX className="size-4" />
        Abort meeting…
      </Button>
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

      <AlertDialog onOpenChange={setAbortOpen} open={abortOpen}>
        <AlertDialogContent data-testid="meeting-host-abort-dialog">
          <AlertDialogHeader>
            <AlertDialogTitle>Abort this meeting?</AlertDialogTitle>
            <AlertDialogDescription>
              Abort is a terminal outcome for a discussion that cannot
              successfully conclude. It does not roll back external effects.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="space-y-3">
            <select
              aria-label="Meeting abort category"
              className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
              disabled={disabled}
              onChange={(event) =>
                setAbortReasonCode(event.target.value as MeetingAbortReason)
              }
              value={abortReasonCode}
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
              onChange={(event) => setAbortReason(event.target.value)}
              placeholder="Optional explanation for participants"
              rows={3}
              value={abortReason}
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
              data-testid="meeting-host-abort-confirm"
              disabled={disabled}
              onClick={async (event) => {
                event.preventDefault();
                const result = await submit({
                  type: "abort",
                  reasonCode: abortReasonCode,
                  reason: abortReason.trim() || undefined,
                });
                if (result?.status === "accepted") setAbortOpen(false);
              }}
            >
              Abort permanently
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
