import * as React from "react";
import { CornerUpRight, X } from "lucide-react";

import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingHandoffDismissReason,
  MeetingHostAction,
  MeetingHostActionResult,
  MeetingOpenHandoff,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Textarea } from "@/shared/ui/textarea";

type SubmitHostAction = (
  action: MeetingHostAction,
) => Promise<MeetingHostActionResult | undefined>;

type DismissDraft = {
  handoffId: string;
  reasonCode: MeetingHandoffDismissReason;
  reason: string;
};

const DISMISS_LABELS: Record<MeetingHandoffDismissReason, string> = {
  superseded: "Superseded",
  answered_elsewhere: "Answered elsewhere",
  out_of_scope: "Out of scope",
  no_longer_needed: "No longer needed",
};

function participantName(
  pubkey: string,
  profiles: Record<string, UserProfileSummary>,
): string {
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

function handoffStatus(handoff: MeetingOpenHandoff): string {
  if (handoff.attemptActive) return "Active Offer or Grant";
  if (handoff.moderatorRetryBlocked) return "Retry blocked";
  if (handoff.blockedBy) return `Blocked by ${handoff.blockedBy}`;
  if (handoff.lastAttemptOutcome) {
    return `Last attempt: ${handoff.lastAttemptOutcome.replaceAll("_", " ")}`;
  }
  return "Open";
}

export function MeetingHostHandoffList({
  disabled,
  hasSelfIntent,
  profiles,
  selectionEnabled,
  snapshot,
  submit,
}: {
  disabled: boolean;
  hasSelfIntent: boolean;
  profiles: Record<string, UserProfileSummary>;
  selectionEnabled: boolean;
  snapshot: MeetingSnapshot;
  submit: SubmitHostAction;
}) {
  const [dismissing, setDismissing] = React.useState<DismissDraft | null>(null);
  const handoffs = snapshot.host?.openHandoffs ?? [];

  if (handoffs.length === 0) return null;

  return (
    <div className="space-y-3" data-testid="meeting-host-handoffs">
      <div className="flex items-center justify-between gap-2">
        <div>
          <h3 className="text-sm font-semibold">Open handoffs</h3>
          <p className="text-xs text-muted-foreground">
            Directed questions that still need an authoritative outcome.
          </p>
        </div>
        <Badge variant="secondary">{handoffs.length}</Badge>
      </div>
      {handoffs.map((handoff) => {
        const dismissDraft =
          dismissing?.handoffId === handoff.handoffId ? dismissing : null;
        const canSelect =
          Boolean(snapshot.host?.canSelect) &&
          selectionEnabled &&
          handoff.selectable &&
          !hasSelfIntent;
        return (
          <article
            className="rounded-lg border p-3"
            data-testid={`meeting-host-handoff-${handoff.handoffId}`}
            key={handoff.handoffId}
          >
            <div className="flex items-start gap-3">
              <CornerUpRight className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium">
                  {participantName(handoff.fromPubkey, profiles)} →{" "}
                  {participantName(handoff.toPubkey, profiles)}
                </p>
                <p className="mt-1 text-sm">{handoff.reasonText}</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  {handoff.reasonType.replaceAll("_", " ")} ·{" "}
                  {handoffStatus(handoff)} · {handoff.attemptCount} attempt
                  {handoff.attemptCount === 1 ? "" : "s"}
                </p>
                <p
                  className="mt-1 text-2xs text-muted-foreground"
                  title={handoff.sourceSpeechEventId}
                >
                  Source Speech {handoff.sourceSpeechEventId.slice(0, 10)}…
                </p>
              </div>
            </div>
            {dismissDraft ? (
              <div className="mt-3 space-y-2 border-t pt-3">
                <select
                  aria-label="Handoff dismissal category"
                  className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
                  disabled={disabled}
                  onChange={(event) =>
                    setDismissing({
                      ...dismissDraft,
                      reasonCode: event.target
                        .value as MeetingHandoffDismissReason,
                    })
                  }
                  value={dismissDraft.reasonCode}
                >
                  {Object.entries(DISMISS_LABELS).map(([value, label]) => (
                    <option key={value} value={value}>
                      {label}
                    </option>
                  ))}
                </select>
                <Textarea
                  aria-label="Handoff dismissal explanation"
                  disabled={disabled}
                  maxLength={1024}
                  onChange={(event) =>
                    setDismissing({
                      ...dismissDraft,
                      reason: event.target.value,
                    })
                  }
                  placeholder="Explain why this handoff should be closed"
                  rows={2}
                  value={dismissDraft.reason}
                />
                <div className="flex justify-end gap-2">
                  <Button
                    disabled={disabled}
                    onClick={() => setDismissing(null)}
                    size="sm"
                    variant="ghost"
                  >
                    <X className="size-4" />
                    Cancel
                  </Button>
                  <Button
                    data-testid="meeting-host-handoff-dismiss-confirm"
                    disabled={disabled || !dismissDraft.reason.trim()}
                    onClick={async () => {
                      const result = await submit({
                        type: "dismiss_handoff",
                        handoffId: handoff.handoffId,
                        reasonCode: dismissDraft.reasonCode,
                        reason: dismissDraft.reason,
                      });
                      if (result?.status === "accepted") setDismissing(null);
                    }}
                    size="sm"
                    variant="destructive"
                  >
                    Dismiss handoff
                  </Button>
                </div>
              </div>
            ) : (
              <div className="mt-3 flex justify-end gap-2">
                {!handoff.attemptActive ? (
                  <Button
                    data-testid="meeting-host-handoff-dismiss"
                    disabled={disabled}
                    onClick={() =>
                      setDismissing({
                        handoffId: handoff.handoffId,
                        reasonCode: "no_longer_needed",
                        reason: "",
                      })
                    }
                    size="sm"
                    variant="ghost"
                  >
                    Dismiss
                  </Button>
                ) : null}
                <Button
                  data-testid="meeting-host-handoff-select"
                  disabled={disabled || !canSelect}
                  onClick={() =>
                    void submit({
                      type: "select_handoff",
                      handoffId: handoff.handoffId,
                    })
                  }
                  size="sm"
                >
                  Retry handoff
                </Button>
              </div>
            )}
          </article>
        );
      })}
    </div>
  );
}
