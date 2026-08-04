import * as React from "react";
import { MessageSquarePlus, RefreshCw, Send, Trash2, X } from "lucide-react";

import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingHostAction,
  MeetingHostActionResult,
  MeetingIntentRejectionReason,
  MeetingPendingIntent,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Textarea } from "@/shared/ui/textarea";

type SubmitHostAction = (
  action: MeetingHostAction,
) => Promise<MeetingHostActionResult | undefined>;

type RejectDraft = {
  intentId: string;
  reasonCode: MeetingIntentRejectionReason;
  reason: string;
};

const REJECTION_LABELS: Record<MeetingIntentRejectionReason, string> = {
  off_topic: "Off topic",
  duplicate: "Duplicate",
  superseded: "Superseded by newer discussion",
  unsupported: "Cannot be supported in this meeting",
  agenda_mismatch: "Outside the current agenda",
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

function ParticipantSelect({
  currentPubkey,
  disabled,
  onChange,
  profiles,
  snapshot,
  value,
}: {
  currentPubkey: string;
  disabled: boolean;
  onChange: (value: string) => void;
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
  value: string;
}) {
  return (
    <select
      aria-label="Address self Intent to a participant"
      className="h-9 min-w-0 rounded-md border border-input bg-background px-3 text-sm shadow-xs outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
      disabled={disabled}
      onChange={(event) => onChange(event.target.value)}
      value={value}
    >
      <option value="">No specific addressee</option>
      {snapshot.participants
        .filter((participant) => participant.pubkey !== currentPubkey)
        .map((participant) => (
          <option key={participant.pubkey} value={participant.pubkey}>
            {participantName(participant.pubkey, profiles)}
          </option>
        ))}
    </select>
  );
}

function SelfIntentCard({
  currentPubkey,
  disabled,
  intent,
  profiles,
  snapshot,
  selectionEnabled,
  submit,
}: {
  currentPubkey: string;
  disabled: boolean;
  intent: MeetingPendingIntent;
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
  selectionEnabled: boolean;
  submit: SubmitHostAction;
}) {
  const [summary, setSummary] = React.useState(intent.summary);
  const [addressedTo, setAddressedTo] = React.useState(
    intent.addressedTo ?? "",
  );
  const [selectionReason, setSelectionReason] = React.useState("");
  const [deferralReason, setDeferralReason] = React.useState("");

  const changed =
    summary.trim() !== intent.summary ||
    addressedTo !== (intent.addressedTo ?? "");
  const otherSelectable = snapshot.host?.pendingIntents.some(
    (candidate) =>
      candidate.authorPubkey !== currentPubkey &&
      candidate.selectable &&
      !candidate.deferred,
  );
  const needsDeferral =
    Boolean(otherSelectable) &&
    (snapshot.host?.consecutiveModeratorSpeeches ?? 0) >= 1;
  const canSelect = Boolean(
    selectionEnabled && snapshot.host?.canSelect && intent.selectable,
  );

  return (
    <article
      className="rounded-lg border border-blue-500/35 bg-blue-500/5 p-3"
      data-testid="meeting-host-self-intent"
    >
      <div className="flex items-center justify-between gap-2">
        <div>
          <p className="text-sm font-medium">Your speaking intent</p>
          <p className="text-xs text-muted-foreground">
            Select it before accepting your own floor Offer.
          </p>
        </div>
        <Badge variant="info">Self</Badge>
      </div>
      <div className="mt-3 grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto]">
        <Input
          aria-label="Self Intent summary"
          data-testid="meeting-self-intent-summary"
          disabled={disabled}
          maxLength={512}
          onChange={(event) => setSummary(event.target.value)}
          value={summary}
        />
        <ParticipantSelect
          currentPubkey={currentPubkey}
          disabled={disabled}
          onChange={setAddressedTo}
          profiles={profiles}
          snapshot={snapshot}
          value={addressedTo}
        />
      </div>
      {canSelect ? (
        <div className="mt-2 grid gap-2">
          <Input
            aria-label="Self selection reason"
            disabled={disabled}
            maxLength={512}
            onChange={(event) => setSelectionReason(event.target.value)}
            placeholder="Optional selection note"
            value={selectionReason}
          />
          {needsDeferral ? (
            <Textarea
              aria-label="Self speech deferral reason"
              data-testid="meeting-self-intent-deferral"
              disabled={disabled}
              maxLength={1024}
              onChange={(event) => setDeferralReason(event.target.value)}
              placeholder="Explain why other eligible intents should wait"
              rows={2}
              value={deferralReason}
            />
          ) : null}
        </div>
      ) : null}
      <div className="mt-3 flex flex-wrap justify-end gap-2">
        <Button
          data-testid="meeting-self-intent-withdraw"
          disabled={disabled}
          onClick={() =>
            void submit({ type: "intent_withdraw", intentId: intent.intentId })
          }
          size="sm"
          variant="ghost"
        >
          <Trash2 className="size-4" />
          Withdraw
        </Button>
        <Button
          data-testid="meeting-self-intent-refresh"
          disabled={disabled || !changed || !summary.trim()}
          onClick={() =>
            void submit({
              type: "intent_refresh",
              intentId: intent.intentId,
              summary,
              addressedTo: addressedTo || undefined,
            })
          }
          size="sm"
          variant="outline"
        >
          <RefreshCw className="size-4" />
          Refresh intent
        </Button>
        {canSelect ? (
          <Button
            data-testid="meeting-self-intent-select"
            disabled={disabled || (needsDeferral && !deferralReason.trim())}
            onClick={() =>
              void submit({
                type: "select_intent",
                intentId: intent.intentId,
                selectionReason: selectionReason.trim() || undefined,
                deferralReason: deferralReason.trim() || undefined,
              })
            }
            size="sm"
          >
            <Send className="size-4" />
            Offer floor to myself
          </Button>
        ) : null}
      </div>
    </article>
  );
}

export function MeetingHostIntentList({
  currentPubkey,
  disabled,
  profiles,
  selectionEnabled,
  snapshot,
  submit,
}: {
  currentPubkey: string;
  disabled: boolean;
  profiles: Record<string, UserProfileSummary>;
  selectionEnabled: boolean;
  snapshot: MeetingSnapshot;
  submit: SubmitHostAction;
}) {
  const host = snapshot.host;
  const selfIntent = host?.pendingIntents.find(
    (intent) => intent.authorPubkey === currentPubkey,
  );
  const otherIntents =
    host?.pendingIntents.filter(
      (intent) => intent.authorPubkey !== currentPubkey,
    ) ?? [];
  const [selfSummary, setSelfSummary] = React.useState("");
  const [selfAddressedTo, setSelfAddressedTo] = React.useState("");
  const [rejecting, setRejecting] = React.useState<RejectDraft | null>(null);

  if (!host) return null;

  return (
    <div className="space-y-3" data-testid="meeting-host-intents">
      <div className="flex items-center justify-between gap-2">
        <div>
          <h3 className="text-sm font-semibold">Speaking intents</h3>
          <p className="text-xs text-muted-foreground">
            Intents are proposed topics, not draft speeches.
          </p>
        </div>
        <Badge variant="secondary">{host.pendingIntents.length}</Badge>
      </div>

      {selfIntent ? (
        <SelfIntentCard
          currentPubkey={currentPubkey}
          disabled={disabled}
          intent={selfIntent}
          key={selfIntent.currentEventId}
          profiles={profiles}
          snapshot={snapshot}
          selectionEnabled={selectionEnabled}
          submit={submit}
        />
      ) : host.canSelect && selectionEnabled ? (
        <div className="rounded-lg border border-dashed p-3">
          <div className="flex items-center gap-2">
            <MessageSquarePlus className="size-4 text-muted-foreground" />
            <p className="text-sm font-medium">I want to speak</p>
          </div>
          <div className="mt-2 grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto]">
            <Input
              aria-label="New self Intent summary"
              data-testid="meeting-self-intent-new-summary"
              disabled={disabled}
              maxLength={512}
              onChange={(event) => setSelfSummary(event.target.value)}
              placeholder="One-sentence speaking intent"
              value={selfSummary}
            />
            <ParticipantSelect
              currentPubkey={currentPubkey}
              disabled={disabled}
              onChange={setSelfAddressedTo}
              profiles={profiles}
              snapshot={snapshot}
              value={selfAddressedTo}
            />
          </div>
          <div className="mt-2 flex justify-end">
            <Button
              data-testid="meeting-self-intent-submit"
              disabled={disabled || !selfSummary.trim()}
              onClick={async () => {
                const result = await submit({
                  type: "intent_submit",
                  summary: selfSummary,
                  addressedTo: selfAddressedTo || undefined,
                });
                if (result?.status === "accepted") setSelfSummary("");
              }}
              size="sm"
              variant="outline"
            >
              Create self intent
            </Button>
          </div>
        </div>
      ) : null}

      {otherIntents.length === 0 ? (
        <p className="rounded-lg border border-dashed px-3 py-4 text-center text-xs text-muted-foreground">
          No participant intents are pending.
        </p>
      ) : (
        otherIntents.map((intent) => {
          const stale = intent.basisSpeechRevision < snapshot.speechRevision;
          const canSelect =
            host.canSelect &&
            selectionEnabled &&
            intent.selectable &&
            selfIntent === undefined;
          const rejectDraft =
            rejecting?.intentId === intent.intentId ? rejecting : null;
          return (
            <article
              className="rounded-lg border p-3"
              data-testid={`meeting-host-intent-${intent.intentId}`}
              key={intent.intentId}
            >
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <p className="text-sm font-medium">
                    {participantName(intent.authorPubkey, profiles)}
                  </p>
                  <p className="mt-1 text-sm">{intent.summary}</p>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {intent.addressedTo
                      ? `Addressed to ${participantName(intent.addressedTo, profiles)}`
                      : "Open to the meeting"}
                    {intent.selectionAttemptCount > 0
                      ? ` · ${intent.selectionAttemptCount} prior attempt${intent.selectionAttemptCount === 1 ? "" : "s"}`
                      : ""}
                  </p>
                </div>
                {stale ? <Badge variant="warning">May be stale</Badge> : null}
              </div>
              {rejectDraft ? (
                <div className="mt-3 space-y-2 border-t pt-3">
                  <select
                    aria-label="Intent rejection category"
                    className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
                    disabled={disabled}
                    onChange={(event) =>
                      setRejecting({
                        ...rejectDraft,
                        reasonCode: event.target
                          .value as MeetingIntentRejectionReason,
                      })
                    }
                    value={rejectDraft.reasonCode}
                  >
                    {Object.entries(REJECTION_LABELS).map(([value, label]) => (
                      <option key={value} value={value}>
                        {label}
                      </option>
                    ))}
                  </select>
                  <Textarea
                    aria-label="Intent rejection explanation"
                    disabled={disabled}
                    maxLength={1024}
                    onChange={(event) =>
                      setRejecting({
                        ...rejectDraft,
                        reason: event.target.value,
                      })
                    }
                    placeholder="Explain why this intent no longer applies"
                    rows={2}
                    value={rejectDraft.reason}
                  />
                  <div className="flex justify-end gap-2">
                    <Button
                      disabled={disabled}
                      onClick={() => setRejecting(null)}
                      size="sm"
                      variant="ghost"
                    >
                      <X className="size-4" />
                      Cancel
                    </Button>
                    <Button
                      data-testid="meeting-host-intent-reject-confirm"
                      disabled={disabled || !rejectDraft.reason.trim()}
                      onClick={async () => {
                        const result = await submit({
                          type: "reject_intent",
                          intentId: intent.intentId,
                          reasonCode: rejectDraft.reasonCode,
                          reason: rejectDraft.reason,
                        });
                        if (result?.status === "accepted") setRejecting(null);
                      }}
                      size="sm"
                      variant="destructive"
                    >
                      Reject intent
                    </Button>
                  </div>
                </div>
              ) : (
                <div className="mt-3 flex justify-end gap-2">
                  <Button
                    data-testid="meeting-host-intent-reject"
                    disabled={disabled}
                    onClick={() =>
                      setRejecting({
                        intentId: intent.intentId,
                        reasonCode: "off_topic",
                        reason: "",
                      })
                    }
                    size="sm"
                    variant="ghost"
                  >
                    Reject
                  </Button>
                  <Button
                    data-testid="meeting-host-intent-select"
                    disabled={disabled || !canSelect}
                    onClick={() =>
                      void submit({
                        type: "select_intent",
                        intentId: intent.intentId,
                      })
                    }
                    size="sm"
                  >
                    Invite {participantName(intent.authorPubkey, profiles)}
                  </Button>
                </div>
              )}
            </article>
          );
        })
      )}
    </div>
  );
}
