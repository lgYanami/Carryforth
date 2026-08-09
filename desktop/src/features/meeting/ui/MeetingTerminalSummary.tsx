import { AlertTriangle, CheckCircle2, XCircle } from "lucide-react";

import type { UserProfileSummary } from "@/shared/api/types";
import type { MeetingEndState } from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";

function endedByLabel(
  pubkey: string,
  profiles: Record<string, UserProfileSummary>,
): string {
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

function terminationSourceLabel(
  source: MeetingEndState["terminationSource"],
): string {
  switch (source) {
    case "host":
      return "Host";
    case "relay":
      return "Community Relay";
    case "unknown":
      return "Unknown signer";
  }
}

function abortCategoryLabel(reasonCode: string | null): string {
  switch (reasonCode) {
    case "goal_unreachable":
      return "Goal unreachable";
    case "insufficient_information":
      return "Insufficient information";
    case "discussion_blocked":
      return "Discussion blocked";
    case "unable_to_form_conclusion":
      return "Unable to form a conclusion";
    case "moderator_unable_to_continue":
      return "Host unable to continue";
    case "participant_revoked":
      return "Participant access revoked";
    default:
      return "Other abort reason";
  }
}

export function MeetingTerminalSummary({
  actionStarted,
  end,
  profiles,
  summary,
}: {
  actionStarted: boolean;
  end: MeetingEndState;
  profiles: Record<string, UserProfileSummary>;
  summary: string | null;
}) {
  const closed = end.outcome === "closed";
  const normalCloseDescription =
    end.terminationSource === "host"
      ? "The host judged that the meeting goal was reached."
      : "The meeting has a normal goal-reached outcome.";

  return (
    <section
      aria-label="Meeting outcome"
      className="shrink-0 border-b bg-background px-4 py-3"
      data-testid="meeting-terminal-summary"
    >
      <div className="flex items-start gap-3">
        {closed ? (
          <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-emerald-500" />
        ) : (
          <XCircle className="mt-0.5 size-4 shrink-0 text-destructive" />
        )}
        <div className="min-w-0 flex-1 space-y-2">
          <div className="flex flex-wrap items-start justify-between gap-2">
            <div className="min-w-0">
              <p className="text-sm font-medium">
                {closed ? "Meeting completed" : "Meeting aborted"}
              </p>
              <p className="text-xs text-muted-foreground">
                {closed
                  ? normalCloseDescription
                  : "The meeting did not end as a normal goal-reached close."}
              </p>
            </div>
            {closed ? (
              <Badge variant={end.actionsAttested ? "success" : "secondary"}>
                {end.actionsAttested
                  ? "Action output confirmed"
                  : "Direct normal close"}
              </Badge>
            ) : (
              <Badge variant="destructive">
                {abortCategoryLabel(end.reasonCode)}
              </Badge>
            )}
          </div>

          <p className="text-2xs text-muted-foreground">
            Ended by {endedByLabel(end.endedBy, profiles)} ·{" "}
            {new Date(end.endedAt * 1_000).toLocaleString()} · Source:{" "}
            {terminationSourceLabel(end.terminationSource)}
          </p>

          {closed ? (
            <p className="text-xs text-muted-foreground">
              {end.actionsAttested
                ? "The close confirms that the final Board's action output was recorded. This does not mean that the resulting work is complete."
                : "The meeting closed without an actions-recorded confirmation."}
            </p>
          ) : end.reason ? (
            <p className="rounded-md bg-muted/50 px-2.5 py-2 text-xs">
              {end.reason}
            </p>
          ) : null}

          {summary ? (
            <div
              className="rounded-md border bg-muted/30 px-2.5 py-2"
              data-testid="meeting-terminal-retrieval-summary"
            >
              <p className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                Retrieval summary
              </p>
              <p className="mt-1 whitespace-pre-wrap text-sm">{summary}</p>
            </div>
          ) : null}

          {!closed && actionStarted ? (
            <div
              className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 px-2.5 py-2 text-xs"
              data-testid="meeting-terminal-external-effects-warning"
            >
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-amber-500" />
              <p>
                Action finalization had started. External system effects may
                remain; Meeting does not verify or list them.
              </p>
            </div>
          ) : null}
        </div>
      </div>
    </section>
  );
}
