import { BotActivityComposerAction } from "@/features/channels/ui/BotActivityBar";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { MeetingAgentActivityAgent } from "@/features/meeting/meetingAgentActivityModel";

export function MeetingAgentActivityDock({
  agents,
  meetingId,
  onOpenAgentActivity,
  openAgentPubkey,
  profiles,
}: {
  agents: readonly MeetingAgentActivityAgent[];
  meetingId: string;
  onOpenAgentActivity: (pubkey: string) => void;
  openAgentPubkey: string | null;
  profiles: UserProfileLookup;
}) {
  if (agents.length === 0) {
    return null;
  }

  return (
    <div
      className="shrink-0 border-t bg-background px-4 py-1.5"
      data-testid="meeting-agent-activity-row"
    >
      <BotActivityComposerAction
        agents={[...agents]}
        channelId={meetingId}
        onOpenAgentSession={onOpenAgentActivity}
        openAgentSessionPubkey={openAgentPubkey}
        profiles={profiles}
        variant="inline"
        workingBotPubkeys={agents.map((agent) => agent.pubkey)}
      />
    </div>
  );
}
