import {
  ArrowUpRight,
  Bot,
  CalendarClock,
  CircleHelp,
  ShieldCheck,
  UserRound,
} from "lucide-react";

import { useMeetingContextDetail } from "@/features/meeting/hooks";
import { useUsersBatchQuery } from "@/features/profile/hooks";
import type { ProjectContextMeetingDetail } from "@/shared/api/tauriProjectContext";
import type { MeetingParticipant } from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

function unixDateTime(value: number | undefined) {
  return value === undefined
    ? "Unknown"
    : dateTimeFormatter.format(new Date(value * 1_000));
}

function isoDateTime(value: string | undefined) {
  if (!value) return "Unknown";
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? value : dateTimeFormatter.format(date);
}

function participantIcon(type: MeetingParticipant["participantType"]) {
  if (type === "agent") return Bot;
  if (type === "human") return UserRound;
  return CircleHelp;
}

/** On-demand full-roster enrichment for one selected Meeting Coordinate. */
export function ProjectContextMeetingContent({
  fallback,
  meetingId,
  onOpenMeeting,
  title,
}: {
  fallback?: ProjectContextMeetingDetail;
  meetingId: string;
  onOpenMeeting: (meetingId: string) => void;
  title?: string;
}) {
  const detailQuery = useMeetingContextDetail(meetingId);
  const verified =
    detailQuery.data?.status === "ready" ? detailQuery.data.detail : undefined;
  const participants: MeetingParticipant[] = verified
    ? verified.participants
    : (fallback?.participantPreview.map((participant) => ({
        ...participant,
        channelRole: "member",
      })) ?? []);
  const hostPubkey = verified?.hostPubkey ?? fallback?.hostPubkey;
  const profilePubkeys = [
    ...new Set([
      ...(hostPubkey ? [hostPubkey] : []),
      ...participants.map((participant) => participant.pubkey),
    ]),
  ].sort();
  const profilesQuery = useUsersBatchQuery(profilePubkeys);
  const profileName = (pubkey: string) =>
    profilesQuery.data?.profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey);
  const terminalOutcome =
    verified?.terminalOutcome ?? fallback?.terminalOutcome;
  const lifecycle =
    verified?.lifecycle ??
    fallback?.lifecycle ??
    (terminalOutcome === "aborted" ? "aborted" : "closed");
  const finalizing = lifecycle === "finalizing_actions";
  const lifecycleLabel = finalizing
    ? "Finalizing actions"
    : lifecycle === "aborted"
      ? "Aborted"
      : lifecycle === "closed"
        ? "Closed"
        : "In progress";
  const action = verified?.actionFinalization;
  const actionSummary = action
    ? action.terminalStatus || action.condition
    : fallback?.actionFinalization?.terminalStatus ||
      fallback?.actionFinalization?.condition;
  const actionsAttested =
    verified?.actionFinalization?.actionsAttested ??
    fallback?.actionFinalization?.actionsAttested;

  return (
    <div className="space-y-5" data-testid="project-context-meeting-detail">
      <section>
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="outline">Meeting</Badge>
          <Badge
            variant={
              finalizing || lifecycle === "aborted" ? "warning" : "success"
            }
          >
            {lifecycleLabel}
          </Badge>
        </div>
        <h3 className="mt-2 text-lg font-semibold leading-tight">
          {verified?.title ?? title ?? "Meeting record"}
        </h3>
        <p className="mt-1 text-sm leading-relaxed text-muted-foreground">
          {verified?.description ??
            fallback?.discussionGoal ??
            "No discussion goal was recorded."}
        </p>
        {finalizing ? (
          <p className="mt-2 rounded-lg border border-warning/30 bg-warning/10 px-3 py-2 text-xs text-muted-foreground">
            Formal discussion and the Board are frozen; Meeting closure is
            pending.
          </p>
        ) : null}
        <Button
          className="mt-3"
          data-testid="project-context-open-meeting"
          onClick={() => onOpenMeeting(meetingId)}
          size="sm"
          type="button"
          variant="outline"
        >
          Open Meeting
          <ArrowUpRight />
        </Button>
      </section>

      <section className="grid grid-cols-2 gap-3 rounded-xl border border-border/70 bg-muted/20 p-3">
        <div>
          <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Created
          </div>
          <div className="mt-1 text-sm">
            {verified
              ? unixDateTime(verified.createdAt)
              : isoDateTime(fallback?.createdAt)}
          </div>
        </div>
        <div>
          <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            {finalizing ? "Lifecycle" : "Ended"}
          </div>
          <div className="mt-1 text-sm">
            {finalizing
              ? "Pending closure"
              : verified
                ? unixDateTime(verified.endedAt ?? undefined)
                : isoDateTime(fallback?.endedAt ?? undefined)}
          </div>
        </div>
        <div className="col-span-2">
          <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Host
          </div>
          <div className="mt-1 break-all text-sm">
            {hostPubkey ? profileName(hostPubkey) : "Unknown"}
          </div>
        </div>
      </section>

      <section className="space-y-2">
        <div className="flex items-center justify-between">
          <h3 className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Frozen roster
          </h3>
          <Badge variant="outline">
            {verified?.participants.length ??
              fallback?.participantCount ??
              participants.length}
          </Badge>
        </div>
        {participants.map((participant) => {
          const Icon = participantIcon(participant.participantType);
          return (
            <div
              className="flex items-center gap-2 rounded-lg border border-border/70 bg-muted/20 px-3 py-2"
              key={participant.pubkey}
            >
              <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate text-sm font-medium">
                {profileName(participant.pubkey)}
              </span>
              <Badge variant="outline">{participant.participantType}</Badge>
            </div>
          );
        })}
        {!verified &&
        fallback &&
        fallback.participantCount > participants.length ? (
          <p className="text-xs text-muted-foreground">
            Open the Meeting to load the complete roster.
          </p>
        ) : null}
      </section>

      <section className="space-y-2 rounded-xl border border-border/70 bg-muted/20 p-3">
        <div className="flex items-center gap-2">
          <ShieldCheck className="h-4 w-4 text-emerald-600 dark:text-emerald-400" />
          <h3 className="text-xs font-semibold">Action Finalization</h3>
        </div>
        <p className="text-sm text-muted-foreground">
          {actionSummary
            ? `${actionSummary}${actionsAttested ? " · output confirmed" : ""}`
            : "No external action run was recorded."}
        </p>
        {detailQuery.isPending ? (
          <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <CalendarClock className="h-3.5 w-3.5" />
            Loading verified Meeting metadata…
          </p>
        ) : null}
      </section>
    </div>
  );
}
