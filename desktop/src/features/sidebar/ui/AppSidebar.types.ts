import type { AddCommunityPrefillRequest } from "@/features/communities/addCommunityPrefill";
import type { Community } from "@/features/communities/types";
import type { useSidebarRelayConnectionCard } from "@/features/sidebar/ui/useSidebarRelayConnectionCard";
import type { SettingsSection } from "@/features/settings/ui/SettingsPanels";
import type { MeetingListItem } from "@/shared/api/tauriMeetings";
import type { SidebarSelectedView } from "@/features/sidebar/ui/AppSidebarPinnedHeader";
import type {
  Channel,
  ChannelVisibility,
  PresenceStatus,
  Profile,
  SearchHit,
  UserStatus,
} from "@/shared/api/types";

export type AppSidebarProps = {
  addCommunityPrefill?: AddCommunityPrefillRequest | null;
  activeCommunity: Community | null;
  channels: Channel[];
  meetingItems: MeetingListItem[];
  currentPubkey?: string;
  fallbackDisplayName?: string;
  homeBadgeCount: number;
  isAddCommunityOpen?: boolean;
  isLoading: boolean;
  isCreatingChannel: boolean;
  isCreatingForum: boolean;
  profile?: Profile;
  relayConnectionCard: ReturnType<typeof useSidebarRelayConnectionCard>;
  selfPresenceStatus: PresenceStatus;
  errorMessage?: string;
  selectedChannelId: string | null;
  selectedView: SidebarSelectedView;
  unreadChannelCounts: ReadonlyMap<string, number>;
  unreadChannelIds: ReadonlySet<string>;
  unreadMeetingIds: ReadonlySet<string>;
  communities: Community[];
  onAddCommunity: (community: Community) => void;
  onAddCommunityOpenChange?: (open: boolean) => void;
  onCreateChannel: (input: {
    name: string;
    description?: string;
    visibility: ChannelVisibility;
    ttlSeconds?: number;
    templateId?: string;
  }) => Promise<void>;
  onCreateForum: (input: {
    name: string;
    description?: string;
    visibility: ChannelVisibility;
    ttlSeconds?: number;
    templateId?: string;
  }) => Promise<void>;
  onOpenAddCommunity: () => void;
  onSendFeedback?: () => void;
  onHideDm: (channelId: string) => void;
  onMarkChannelUnread: (channelId: string) => void;
  onMarkChannelRead: (
    channelId: string,
    lastMessageAt: string | null | undefined,
  ) => void;
  onMarkAllChannelsRead: () => void;
  onBrowseChannels?: (onCreated?: (channelId: string) => void) => void;
  onOpenDm: (input: { pubkeys: string[] }) => Promise<void>;
  onUpdateCommunity: (
    id: string,
    updates: Partial<Pick<Community, "name" | "relayUrl" | "token">>,
  ) => void;
  onRemoveCommunity: (id: string) => void;
  onCreateAgent: () => void;
  onSelectAgents: () => void;
  onSelectCommunity: () => void;
  onSelectProjectContext: () => void;
  onSelectDocuments: () => void;
  onSelectProjects: () => void;
  onSelectPulse: () => void;
  onSelectWorkflows: () => void;
  onSelectHome: () => void;
  onSelectChannel: (channelId: string) => void;
  onOpenSearchResult: (hit: SearchHit) => void;
  /**
   * Full channel set used for global search. Unlike `channels` (which is
   * scoped to the viewer's joined sidebar list), this includes open channels
   * the viewer hasn't joined, so search can surface them.
   */
  searchChannels: Channel[];
  searchFocusRequest: number;
  onSelectSettings: (section?: SettingsSection) => void;
  onSetPresenceStatus?: (status: "online" | "away" | "offline") => void;
  onSetUserStatus: (text: string, emoji: string) => void;
  onClearUserStatus: () => void;
  onSwitchCommunity: (id: string) => void;
  selfUserStatus?: UserStatus;
  isPresencePending?: boolean;
  onNewMessage: () => void;
  onBackgroundClick?: () => void;
  isCreateChannelOpen?: boolean;
  onCreateChannelOpenChange?: (open: boolean) => void;
  mutedChannelIds?: ReadonlySet<string>;
  onMuteChannel?: (channelId: string) => void;
  onUnmuteChannel?: (channelId: string) => void;
  starredChannelIds?: ReadonlySet<string>;
  onStarChannel?: (channelId: string) => void;
  onUnstarChannel?: (channelId: string) => void;
};
