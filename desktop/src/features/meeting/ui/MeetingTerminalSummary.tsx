import { CheckCircle2, XCircle } from "lucide-react";

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

export function MeetingTerminalSummary({
  actionStarted,
  end,
  profiles,
}: {
  actionStarted: boolean;
  end: MeetingEndState;
  profiles: Record<string, UserProfileSummary>;
}) {
  const closed = end.outcome === "closed";
  return (
    <section
      aria-label="Meeting outcome"
      className="flex shrink-0 items-center gap-3 border-b bg-background px-4 py-2.5"
      data-testid="meeting-terminal-summary"
    >
      {closed ? (
        <CheckCircle2 className="size-4 shrink-0 text-emerald-500" />
      ) : (
        <XCircle className="size-4 shrink-0 text-destructive" />
      )}
      <div className="min-w-0 flex-1">
        <p className="truncate text-xs font-medium">
          {closed ? "Meeting completed" : "Meeting aborted"}
        </p>
        <p className="truncate text-2xs text-muted-foreground">
          Ended by {endedByLabel(end.endedBy, profiles)} ·{" "}
          {new Date(end.endedAt * 1_000).toLocaleString()}
          {!closed && end.reasonCode
            ? ` · ${end.reasonCode.replaceAll("_", " ")}`
            : ""}
        </p>
      </div>
      {closed ? (
        <Badge variant={end.actionsAttested ? "success" : "secondary"}>
          {end.actionsAttested ? "Actions recorded" : "Closed directly"}
        </Badge>
      ) : actionStarted ? (
        <Badge variant="warning">External effects may remain</Badge>
      ) : null}
    </section>
  );
}
