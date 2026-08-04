import * as React from "react";
import { Clock3, Handshake } from "lucide-react";

import type {
  MeetingFloorAction,
  MeetingOffer,
} from "@/shared/api/tauriMeetings";
import { Button } from "@/shared/ui/button";
import { Textarea } from "@/shared/ui/textarea";

type MeetingOfferControlsProps = {
  disabled: boolean;
  offer: MeetingOffer;
  onDeadline: () => void;
  onSubmit: (action: MeetingFloorAction) => Promise<void>;
};

function sourceLabel(source: MeetingOffer["allocationSource"]): string {
  switch (source) {
    case "human_request":
      return "Your request reached the front of the queue";
    case "directed_handoff":
      return "A participant asked you to respond";
    case "moderator_select":
      return "The host selected you";
    case "fallback":
      return "The meeting selected you as the next speaker";
  }
}

function useDeadline(deadlineMs: number, onDeadline: () => void) {
  const [now, setNow] = React.useState(Date.now());
  const firedFor = React.useRef<number | null>(null);
  const onDeadlineEvent = React.useEffectEvent(onDeadline);

  React.useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);

  const remainingMs = Math.max(0, deadlineMs - now);
  React.useEffect(() => {
    if (remainingMs > 0 || firedFor.current === deadlineMs) return;
    firedFor.current = deadlineMs;
    onDeadlineEvent();
  }, [deadlineMs, remainingMs]);
  return remainingMs;
}

function remainingLabel(remainingMs: number): string {
  if (remainingMs <= 0) return "Checking authoritative state…";
  const seconds = Math.ceil(remainingMs / 1_000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export function MeetingOfferControls({
  disabled,
  offer,
  onDeadline,
  onSubmit,
}: MeetingOfferControlsProps) {
  const [declining, setDeclining] = React.useState(false);
  const [reason, setReason] = React.useState("");
  const remainingMs = useDeadline(offer.ackDeadlineMs, onDeadline);
  const expired = remainingMs <= 0;

  return (
    <div
      className="mx-auto w-full max-w-3xl rounded-xl border border-blue-500/35 bg-blue-500/5 p-4"
      data-testid="meeting-offer-controls"
    >
      <div className="flex items-start gap-3">
        <div className="rounded-full bg-blue-500/10 p-2 text-blue-600 dark:text-blue-400">
          <Handshake className="size-5" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 className="text-sm font-semibold">It is your turn to speak</h2>
            <span className="flex items-center gap-1 text-xs text-muted-foreground">
              <Clock3 className="size-3.5" />
              {remainingLabel(remainingMs)}
            </span>
          </div>
          <p className="mt-1 text-xs text-muted-foreground">
            {sourceLabel(offer.allocationSource)}. The composer opens only after
            the Relay issues a Grant.
          </p>
          {offer.handoffContext ? (
            <blockquote className="mt-3 border-l-2 border-blue-500/50 pl-3 text-sm">
              {offer.handoffContext.reasonText}
            </blockquote>
          ) : null}
          {declining ? (
            <div className="mt-3 space-y-2">
              <Textarea
                aria-label="Offer decline reason"
                data-testid="meeting-offer-decline-reason"
                disabled={disabled || expired}
                maxLength={512}
                onChange={(event) => setReason(event.target.value)}
                placeholder="Optional reason"
                rows={2}
                value={reason}
              />
              <div className="flex justify-end gap-2">
                <Button
                  disabled={disabled}
                  onClick={() => setDeclining(false)}
                  size="sm"
                  variant="ghost"
                >
                  Cancel
                </Button>
                <Button
                  data-testid="meeting-offer-decline-confirm"
                  disabled={disabled || expired}
                  onClick={() =>
                    void onSubmit({
                      type: "offer_decline",
                      reason: reason.trim() || undefined,
                    })
                  }
                  size="sm"
                  variant="outline"
                >
                  Decline offer
                </Button>
              </div>
            </div>
          ) : (
            <div className="mt-3 flex justify-end gap-2">
              <Button
                data-testid="meeting-offer-decline"
                disabled={disabled || expired}
                onClick={() => setDeclining(true)}
                size="sm"
                variant="outline"
              >
                Decline
              </Button>
              <Button
                data-testid="meeting-offer-accept"
                disabled={disabled || expired}
                onClick={() => void onSubmit({ type: "offer_ack" })}
                size="sm"
              >
                Accept floor
              </Button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
