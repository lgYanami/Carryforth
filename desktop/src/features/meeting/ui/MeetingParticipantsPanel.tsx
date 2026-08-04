import { Bot, Crown, UserRound } from "lucide-react";

import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
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

export function MeetingParticipantsPanel({
  profiles,
  snapshot,
}: {
  profiles: Record<string, UserProfileSummary>;
  snapshot: MeetingSnapshot;
}) {
  return (
    <div className="space-y-2" data-testid="meeting-participants">
      {snapshot.participants.map((participant) => {
        const profile = profiles[participant.pubkey.toLowerCase()];
        const name = participantName(participant.pubkey, profiles);
        const isHost = participant.pubkey === snapshot.moderatorPubkey;
        const isSpeaking = participant.pubkey === snapshot.currentSpeakerPubkey;
        return (
          <div
            className="flex items-center gap-3 rounded-lg border border-border/60 px-3 py-2"
            data-testid={`meeting-participant-${participant.pubkey}`}
            key={participant.pubkey}
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
                ) : (
                  <UserRound className="size-3" />
                )}
                <span className="capitalize">
                  {participant.participantType}
                </span>
                <span aria-hidden="true">·</span>
                <span className="truncate">{participant.channelRole}</span>
              </div>
            </div>
            {isSpeaking ? <Badge variant="info">Speaking</Badge> : null}
          </div>
        );
      })}
    </div>
  );
}
