import {
  ArrowRight,
  ClipboardCheck,
  Clock3,
  Eye,
  MessageSquareText,
} from "lucide-react";

import {
  agentHostActionStatus,
  agentHostBoardOutcomeLabel,
  agentHostHandoffStatus,
  agentHostIntentStatus,
  agentHostPhasePresentation,
} from "@/features/meeting/agentHostObservationModel";
import { meetingParticipantStatus } from "@/features/meeting/participantPresentation";
import {
  meetingDeadlineLabel,
  useMeetingDeadline,
} from "@/features/meeting/useMeetingDeadline";
import type { UserProfileSummary } from "@/shared/api/types";
import type {
  MeetingOpenHandoff,
  MeetingPendingIntent,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";
import { MeetingParticipantStatusBadge } from "./MeetingParticipantStatusBadge";

function participantName(
  pubkey: string | null | undefined,
  profiles: Record<string, UserProfileSummary>,
): string {
  if (!pubkey) return "a participant";
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

function IntentObservationItem({
  intent,
  profiles,
  snapshot,
}: {
  intent: MeetingPendingIntent;
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
}) {
  return (
    <article className="rounded-lg border p-3">
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
          </p>
        </div>
        <Badge variant={intent.deferred ? "warning" : "outline"}>
          {agentHostIntentStatus(intent, snapshot)}
        </Badge>
      </div>
    </article>
  );
}

function HandoffObservationItem({
  handoff,
  profiles,
}: {
  handoff: MeetingOpenHandoff;
  profiles: Record<string, UserProfileSummary>;
}) {
  return (
    <article className="rounded-lg border p-3">
      <div className="flex items-start gap-3">
        <ArrowRight className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-sm font-medium">
              {participantName(handoff.fromPubkey, profiles)} →{" "}
              {participantName(handoff.toPubkey, profiles)}
            </p>
            <Badge variant="outline">{agentHostHandoffStatus(handoff)}</Badge>
          </div>
          <p className="mt-1 text-sm">{handoff.reasonText}</p>
          <p className="mt-1 text-xs capitalize text-muted-foreground">
            {handoff.reasonType.replaceAll("_", " ")}
          </p>
        </div>
      </div>
    </article>
  );
}

export function MeetingHostObservation({
  authorityAvailable,
  onRefresh,
  profiles,
  snapshot,
}: {
  authorityAvailable: boolean;
  onRefresh: () => void;
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
}) {
  const phase = agentHostPhasePresentation(snapshot);
  const remainingMs = useMeetingDeadline(phase.deadlineMs, onRefresh);
  const activePubkey =
    snapshot.floor?.grant?.holderPubkey ??
    snapshot.currentSpeakerPubkey ??
    snapshot.floor?.offer?.targetPubkey ??
    snapshot.currentOfferPubkey;
  const activeParticipant = snapshot.participants.find(
    (participant) => participant.pubkey === activePubkey,
  );
  const activeStatus = activeParticipant
    ? meetingParticipantStatus(activeParticipant, snapshot)
    : null;
  const intents = snapshot.host?.pendingIntents ?? [];
  const handoffs = snapshot.host?.openHandoffs ?? [];
  const action = agentHostActionStatus(snapshot);

  return (
    <section
      aria-label="Agent host observation"
      className="mx-auto mb-3 max-h-[40vh] w-full max-w-4xl overflow-y-auto rounded-xl border bg-background p-4 shadow-xs"
      data-host-phase={phase.kind}
      data-testid="meeting-host-observation"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-3">
          <div className="rounded-full bg-blue-500/10 p-2 text-blue-600">
            <Eye className="size-5" />
          </div>
          <div>
            <h2 className="text-sm font-semibold">Agent host progress</h2>
            <p className="mt-0.5 text-xs text-muted-foreground">
              Verified Meeting state for participants. Host controls remain with
              the Agent identity.
            </p>
          </div>
        </div>
        <Badge variant={authorityAvailable ? "info" : "warning"}>
          {authorityAvailable ? "Read only" : "Last verified"}
        </Badge>
      </div>

      <div
        className="mt-4 rounded-lg border bg-muted/20 p-3"
        data-testid="meeting-host-observation-phase"
      >
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div className="min-w-0">
            <p className="text-sm font-medium">{phase.title}</p>
            <p className="mt-1 text-xs text-muted-foreground">
              {phase.description}
            </p>
          </div>
          {remainingMs !== null ? (
            <Badge variant={remainingMs <= 0 ? "warning" : "outline"}>
              <Clock3 className="mr-1 size-3" />
              {meetingDeadlineLabel(remainingMs)}
            </Badge>
          ) : null}
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-2 border-t pt-3 text-xs text-muted-foreground">
          <ClipboardCheck className="size-4" />
          <span>{agentHostBoardOutcomeLabel(snapshot)}</span>
        </div>
      </div>

      {activeParticipant && activeStatus ? (
        <div
          className="mt-3 flex items-center justify-between gap-3 rounded-lg border px-3 py-2"
          data-testid="meeting-host-observation-floor"
        >
          <div className="min-w-0">
            <p className="text-xs text-muted-foreground">Current Floor</p>
            <p className="truncate text-sm font-medium">
              {participantName(activeParticipant.pubkey, profiles)}
            </p>
          </div>
          <MeetingParticipantStatusBadge status={activeStatus} />
        </div>
      ) : null}

      {action ? (
        <div
          className="mt-3 flex items-center gap-3 rounded-lg border px-3 py-2"
          data-testid="meeting-host-observation-action"
        >
          <ClipboardCheck className="size-4 shrink-0 text-muted-foreground" />
          <div>
            <p className="text-xs text-muted-foreground">Action output</p>
            <p className="text-sm font-medium">{action}</p>
          </div>
        </div>
      ) : null}

      <div className="mt-4 grid gap-4 lg:grid-cols-2">
        <div
          className="space-y-2"
          data-testid="meeting-host-observation-intents"
        >
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center gap-2">
              <MessageSquareText className="size-4 text-muted-foreground" />
              <h3 className="text-sm font-semibold">Pending intents</h3>
            </div>
            <Badge variant="secondary">{intents.length}</Badge>
          </div>
          {intents.length ? (
            intents.map((intent) => (
              <IntentObservationItem
                intent={intent}
                key={intent.intentId}
                profiles={profiles}
                snapshot={snapshot}
              />
            ))
          ) : (
            <p className="rounded-lg border border-dashed px-3 py-3 text-xs text-muted-foreground">
              No participant intents are pending.
            </p>
          )}
        </div>
        <div
          className="space-y-2"
          data-testid="meeting-host-observation-handoffs"
        >
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center gap-2">
              <ArrowRight className="size-4 text-muted-foreground" />
              <h3 className="text-sm font-semibold">Open handoffs</h3>
            </div>
            <Badge variant="secondary">{handoffs.length}</Badge>
          </div>
          {handoffs.length ? (
            handoffs.map((handoff) => (
              <HandoffObservationItem
                handoff={handoff}
                key={handoff.handoffId}
                profiles={profiles}
              />
            ))
          ) : (
            <p className="rounded-lg border border-dashed px-3 py-3 text-xs text-muted-foreground">
              No Directed Handoffs are open.
            </p>
          )}
        </div>
      </div>
    </section>
  );
}
