import * as React from "react";
import { AtSign, Clock3, CornerDownRight, Send, X } from "lucide-react";

import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingGrant,
  MeetingGrantYieldReason,
  MeetingHandoffType,
  MeetingParticipant,
} from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import { Checkbox } from "@/shared/ui/checkbox";
import { Textarea } from "@/shared/ui/textarea";

export type MeetingSpeechDraft = {
  content: string;
  mentions: string[];
  handoffTarget: string;
  handoffType: MeetingHandoffType;
  handoffReason: string;
};

type MeetingSpeechComposerProps = {
  disabled: boolean;
  draft: MeetingSpeechDraft;
  grant: MeetingGrant;
  onChange: (draft: MeetingSpeechDraft) => void;
  onDeadline: () => void;
  onSubmit: () => Promise<void>;
  onYield: (reasonCode: MeetingGrantYieldReason) => Promise<void>;
  participants: MeetingParticipant[];
  profiles: Record<string, UserProfileSummary>;
  selfPubkey: string;
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

export function MeetingSpeechComposer({
  disabled,
  draft,
  grant,
  onChange,
  onDeadline,
  onSubmit,
  onYield,
  participants,
  profiles,
  selfPubkey,
}: MeetingSpeechComposerProps) {
  const [now, setNow] = React.useState(Date.now());
  const [showMentions, setShowMentions] = React.useState(false);
  const [showHandoff, setShowHandoff] = React.useState(false);
  const [showYield, setShowYield] = React.useState(false);
  const [yieldReason, setYieldReason] =
    React.useState<MeetingGrantYieldReason>("cancelled");
  const firedDeadline = React.useRef<number | null>(null);
  const onDeadlineEvent = React.useEffectEvent(onDeadline);

  React.useEffect(() => {
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);
  const remainingMs = Math.max(0, grant.hardDeadlineMs - now);
  const expired = remainingMs <= 0;
  React.useEffect(() => {
    if (!expired || firedDeadline.current === grant.hardDeadlineMs) return;
    firedDeadline.current = grant.hardDeadlineMs;
    onDeadlineEvent();
  }, [expired, grant.hardDeadlineMs]);

  const otherParticipants = participants.filter(
    (participant) => participant.pubkey !== selfPubkey,
  );
  const handoffReady =
    !showHandoff ||
    (draft.handoffTarget.length > 0 && draft.handoffReason.trim().length > 0);
  const canSubmit =
    !disabled && !expired && draft.content.trim().length > 0 && handoffReady;

  return (
    <div
      className="mx-auto w-full max-w-3xl rounded-xl border bg-card p-4 shadow-xs"
      data-testid="meeting-speech-composer"
    >
      <div className="mb-2 flex items-center justify-between gap-3">
        <div>
          <h2 className="text-sm font-semibold">You have the floor</h2>
          <p className="text-xs text-muted-foreground">
            One formal Speech can consume this Grant.
          </p>
        </div>
        <span className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
          <Clock3 className="size-3.5" />
          {expired
            ? "Checking authoritative state…"
            : `${Math.max(1, Math.ceil(remainingMs / 1_000))}s`}
        </span>
      </div>
      <Textarea
        aria-label="Formal Meeting Speech"
        data-testid="meeting-speech-input"
        disabled={disabled || expired}
        maxLength={256 * 1024}
        onChange={(event) =>
          onChange({ ...draft, content: event.target.value })
        }
        placeholder="Write your formal contribution in Markdown…"
        rows={5}
        value={draft.content}
      />

      <div className="mt-2 flex flex-wrap gap-2">
        <Button
          aria-pressed={showMentions}
          disabled={disabled || expired}
          onClick={() => setShowMentions((value) => !value)}
          size="sm"
          variant="ghost"
        >
          <AtSign className="size-4" />
          Mentions{" "}
          {draft.mentions.length > 0 ? `(${draft.mentions.length})` : ""}
        </Button>
        <Button
          aria-pressed={showHandoff}
          data-testid="meeting-handoff-toggle"
          disabled={disabled || expired}
          onClick={() => {
            setShowHandoff((value) => !value);
            if (showHandoff) {
              onChange({
                ...draft,
                handoffTarget: "",
                handoffReason: "",
              });
            }
          }}
          size="sm"
          variant="ghost"
        >
          <CornerDownRight className="size-4" />
          Ask someone to respond
        </Button>
      </div>

      {showMentions ? (
        <div
          className="mt-2 flex flex-wrap gap-2 rounded-lg border bg-muted/20 p-2"
          data-testid="meeting-mention-picker"
        >
          {participants.map((participant) => {
            const checked = draft.mentions.includes(participant.pubkey);
            return (
              <label
                className="flex cursor-pointer items-center gap-2 rounded-md px-2 py-1 text-xs hover:bg-muted"
                htmlFor={`meeting-mention-${participant.pubkey}`}
                key={participant.pubkey}
              >
                <Checkbox
                  checked={checked}
                  disabled={disabled || expired}
                  id={`meeting-mention-${participant.pubkey}`}
                  onCheckedChange={(next) =>
                    onChange({
                      ...draft,
                      mentions: next
                        ? [...draft.mentions, participant.pubkey]
                        : draft.mentions.filter(
                            (pubkey) => pubkey !== participant.pubkey,
                          ),
                    })
                  }
                />
                {participantName(participant.pubkey, profiles)}
              </label>
            );
          })}
        </div>
      ) : null}

      {showHandoff ? (
        <div
          className="mt-2 grid gap-2 rounded-lg border bg-muted/20 p-3 sm:grid-cols-2"
          data-testid="meeting-handoff-fields"
        >
          <label className="space-y-1 text-xs font-medium">
            Respondent
            <select
              className="h-9 w-full rounded-md border bg-background px-3 text-sm"
              data-testid="meeting-handoff-target"
              disabled={disabled || expired}
              onChange={(event) =>
                onChange({ ...draft, handoffTarget: event.target.value })
              }
              value={draft.handoffTarget}
            >
              <option value="">Select participant</option>
              {otherParticipants.map((participant) => (
                <option key={participant.pubkey} value={participant.pubkey}>
                  {participantName(participant.pubkey, profiles)}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1 text-xs font-medium">
            Request type
            <select
              className="h-9 w-full rounded-md border bg-background px-3 text-sm"
              disabled={disabled || expired}
              onChange={(event) =>
                onChange({
                  ...draft,
                  handoffType: event.target.value as MeetingHandoffType,
                })
              }
              value={draft.handoffType}
            >
              <option value="question">Question</option>
              <option value="information_request">Information request</option>
              <option value="clarification">Clarification</option>
              <option value="review">Review</option>
              <option value="response_requested">Response requested</option>
            </select>
          </label>
          <label
            className="space-y-1 text-xs font-medium sm:col-span-2"
            htmlFor="meeting-handoff-reason"
          >
            Why this person should respond
            <Textarea
              data-testid="meeting-handoff-reason"
              disabled={disabled || expired}
              id="meeting-handoff-reason"
              maxLength={1024}
              onChange={(event) =>
                onChange({ ...draft, handoffReason: event.target.value })
              }
              rows={2}
              value={draft.handoffReason}
            />
          </label>
        </div>
      ) : null}

      {showYield ? (
        <div className="mt-3 flex flex-wrap items-center justify-end gap-2">
          <select
            aria-label="Yield reason"
            className="h-9 rounded-md border bg-background px-3 text-sm"
            disabled={disabled || expired}
            onChange={(event) =>
              setYieldReason(event.target.value as MeetingGrantYieldReason)
            }
            value={yieldReason}
          >
            <option value="cancelled">I am cancelling</option>
            <option value="no_longer_needed">No longer needed</option>
            <option value="unable_to_answer">Unable to answer</option>
            <option value="insufficient_context">Insufficient context</option>
            <option value="tool_failure">Tool failure</option>
          </select>
          <Button
            data-testid="meeting-yield-confirm"
            disabled={disabled || expired}
            onClick={() => void onYield(yieldReason)}
            size="sm"
            variant="destructive"
          >
            Confirm yield
          </Button>
        </div>
      ) : (
        <div className="mt-3 flex justify-end gap-2">
          <Button
            data-testid="meeting-yield"
            disabled={disabled || expired}
            onClick={() => setShowYield(true)}
            size="sm"
            variant="outline"
          >
            <X className="size-4" />
            Yield floor
          </Button>
          <Button
            data-testid="meeting-speech-submit"
            disabled={!canSubmit}
            onClick={() => void onSubmit()}
            size="sm"
          >
            <Send className="size-4" />
            {showHandoff ? "Publish and request response" : "Publish Speech"}
          </Button>
        </div>
      )}
    </div>
  );
}
