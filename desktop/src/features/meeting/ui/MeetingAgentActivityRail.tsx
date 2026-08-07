import { AgentSessionThreadPanel } from "@/features/channels/ui/AgentSessionThreadPanel";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import type { MeetingAgentActivityAgent } from "@/features/meeting/meetingAgentActivityModel";

export function MeetingAgentActivityRail({
  agent,
  meetingId,
  meetingTitle,
  onBack,
  onClose,
  profiles,
  widthPx,
}: {
  agent: MeetingAgentActivityAgent;
  meetingId: string;
  meetingTitle: string;
  onBack: () => void;
  onClose: () => void;
  profiles: UserProfileLookup;
  widthPx: number;
}) {
  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col"
      data-testid="meeting-agent-activity-rail"
    >
      <AgentSessionThreadPanel
        agent={agent}
        allowInterruptTurn={false}
        canInterruptTurn={false}
        channel={null}
        channelId={meetingId}
        emptyDescription={`No ACP activity has been recorded for ${agent.name} in this Meeting yet.`}
        layout="split"
        onBack={onBack}
        onClose={onClose}
        profiles={profiles}
        scopeLabelOverride={`Meeting · ${meetingTitle}`}
        transparentChrome
        widthPx={widthPx}
      />
    </div>
  );
}
