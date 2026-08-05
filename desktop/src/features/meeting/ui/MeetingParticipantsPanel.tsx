import { Bot, CircleHelp, Crown, UserRound } from "lucide-react";

import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import {
  meetingParticipantGroups,
  type MeetingParticipantPresentation,
  type MeetingParticipantStatusKind,
} from "@/features/meeting/participantPresentation";
import type { UserProfileSummary } from "@/shared/api/types";
import type { MeetingSnapshot } from "@/shared/api/tauriMeetings";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Badge } from "@/shared/ui/badge";

function participantName(
  pubkey: string,
  profiles: Record<string, UserProfileSummary>,
): string {
  return (
    profiles[pubkey.toLowerCase()]?.displayName?.trim() ||
    truncatePubkey(pubkey)
  );
}

const statusVariants: Record<
  MeetingParticipantStatusKind,
  "info" | "warning" | "outline" | "secondary"
> = {
  speaking: "info",
  waiting_for_ack: "warning",
  floor_requested: "outline",
  intent_pending: "outline",
  idle: "secondary",
};

function ParticipantRow({
  presentation,
  profiles,
}: {
  presentation: MeetingParticipantPresentation;
  profiles: Record<string, UserProfileSummary>;
}) {
  const { isHost, participant, status } = presentation;
  const profile = profiles[participant.pubkey.toLowerCase()];
  const name = participantName(participant.pubkey, profiles);
  return (
    <div
      className="flex items-center gap-3 rounded-lg border border-border/60 px-3 py-2"
      data-meeting-status={status.kind}
      data-testid={`meeting-participant-${participant.pubkey}`}
    >
      <ProfileAvatar
        avatarUrl={profile?.avatarUrl ?? null}
        className="size-9"
        label={name}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="truncate text-sm font-medium">{name}</span>
          {isHost ? (
            <Crown
              aria-label="Host"
              className="size-3.5 shrink-0 text-amber-500"
            />
          ) : null}
        </div>
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          {participant.participantType === "agent" ? (
            <Bot className="size-3" />
          ) : participant.participantType === "human" ? (
            <UserRound className="size-3" />
          ) : (
            <CircleHelp className="size-3" />
          )}
          <span className="capitalize">{participant.participantType}</span>
          <span aria-hidden="true">·</span>
          <span className="truncate">{participant.channelRole}</span>
        </div>
      </div>
      <div className="flex shrink-0 flex-col items-end gap-1">
        <Badge
          data-testid={`meeting-participant-status-${participant.pubkey}`}
          variant={statusVariants[status.kind]}
        >
          {status.label}
        </Badge>
        {status.detail ? (
          <span className="text-2xs text-muted-foreground">
            {status.detail}
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function MeetingParticipantsPanel({
  profiles,
  snapshot,
}: {
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
}) {
  const groups = meetingParticipantGroups(snapshot);
  return (
    <div className="space-y-5" data-testid="meeting-participants">
      {groups.map((group) => (
        <section
          className="space-y-2"
          data-testid={`meeting-participant-group-${group.key}`}
          key={group.key}
        >
          <div className="flex items-center justify-between px-1">
            <h3 className="text-xs font-semibold text-muted-foreground">
              {group.label}
            </h3>
            <span className="text-2xs text-muted-foreground">
              {group.participants.length}
            </span>
          </div>
          {group.participants.map((presentation) => (
            <ParticipantRow
              key={presentation.participant.pubkey}
              presentation={presentation}
              profiles={profiles}
            />
          ))}
        </section>
      ))}
    </div>
  );
}
