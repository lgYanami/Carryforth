import type { CommunityDestination } from "@/features/communities/communityNavigationStorage";
import type { Channel } from "@/shared/api/types";

export type CommunityContinueTarget =
  | {
      kind: "home";
      label: "Open Inbox";
    }
  | {
      kind: "channel";
      channelId: string;
      label: string;
    };

export type CommunityContinueResolution = {
  status: "pending" | "ready" | "invalid";
  target: CommunityContinueTarget;
};

const HOME_TARGET: CommunityContinueTarget = {
  kind: "home",
  label: "Open Inbox",
};

/**
 * Resolves a stored work destination only after the active Community's live
 * channel list has been read. Cached snapshots can paint the sidebar, but
 * cannot prove that a remembered channel is still joined and active.
 */
export function resolveCommunityContinueTarget(
  destination: CommunityDestination | null,
  channels: readonly Channel[],
  channelsValidated: boolean,
): CommunityContinueResolution {
  if (destination?.kind !== "channel") {
    return { status: "ready", target: HOME_TARGET };
  }
  if (!channelsValidated) {
    return { status: "pending", target: HOME_TARGET };
  }

  const channel = channels.find(
    (candidate) =>
      candidate.id === destination.channelId &&
      candidate.isMember &&
      candidate.archivedAt === null,
  );
  if (!channel) {
    return { status: "invalid", target: HOME_TARGET };
  }

  return {
    status: "ready",
    target: {
      kind: "channel",
      channelId: channel.id,
      label:
        channel.channelType === "dm"
          ? `Continue ${channel.name}`
          : `Continue in #${channel.name}`,
    },
  };
}
