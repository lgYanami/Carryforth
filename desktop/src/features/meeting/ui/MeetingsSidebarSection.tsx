import * as React from "react";
import {
  CalendarClock,
  ChevronDown,
  History,
  Plus,
  UsersRound,
} from "lucide-react";

import { requestOpenCreateMeeting } from "@/features/meeting/openCreateMeetingEvent";
import type {
  MeetingLifecycle,
  MeetingListItem,
} from "@/shared/api/tauriMeetings";
import { cn } from "@/shared/lib/cn";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui/dialog";
import {
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/shared/ui/sidebar";

type MeetingsSidebarSectionProps = {
  currentPubkey?: string;
  items: MeetingListItem[];
  selectedMeetingId: string | null;
  unreadMeetingIds: ReadonlySet<string>;
  onSelectMeeting: (meetingId: string) => void;
};

const ACTIVE_PAGE_SIZE = 20;
const HISTORY_PAGE_SIZE = 50;

function lifecycleLabel(lifecycle: MeetingLifecycle | null): string {
  switch (lifecycle) {
    case "initializing":
      return "Starting";
    case "active":
      return "In progress";
    case "finalizing_actions":
      return "Recording actions";
    case "closed":
      return "Closed";
    case "aborted":
      return "Aborted";
    default:
      return "Compatibility required";
  }
}

function meetingStatus(item: MeetingListItem): string {
  if (item.compatibility !== "ready") {
    return lifecycleLabel(null);
  }
  if (item.lifecycle === "active" && item.currentSpeakerPubkey) {
    return `Speaking · ${truncatePubkey(item.currentSpeakerPubkey)}`;
  }
  return lifecycleLabel(item.lifecycle);
}

function MeetingRow({
  item,
  needsAttention,
  isSelected,
  isUnread,
  onSelect,
}: {
  item: MeetingListItem;
  needsAttention: boolean;
  isSelected: boolean;
  isUnread: boolean;
  onSelect: () => void;
}) {
  return (
    <SidebarMenuItem data-testid={`meeting-row-${item.meetingId}`}>
      <SidebarMenuButton
        className="h-auto min-h-9 items-start gap-2 py-1.5"
        isActive={isSelected}
        onClick={onSelect}
        tooltip={item.title}
      >
        <UsersRound className="mt-0.5 size-4 shrink-0" />
        <span className="min-w-0 flex-1 group-data-[collapsible=icon]:hidden">
          <span
            className={cn(
              "block truncate text-sm",
              isUnread && "font-semibold",
            )}
          >
            {item.title}
          </span>
          <span className="block truncate text-xs text-sidebar-foreground/55">
            {meetingStatus(item)}
          </span>
        </span>
        <span className="mt-1.5 flex shrink-0 gap-1 group-data-[collapsible=icon]:hidden">
          {isUnread ? (
            <span
              aria-label="Unread Meeting speech"
              className="size-2 rounded-full bg-primary"
              data-testid={`meeting-unread-${item.meetingId}`}
              role="status"
            />
          ) : null}
          {needsAttention ? (
            <span
              aria-label="Meeting needs your attention"
              className="size-2 rounded-full bg-amber-500"
              data-testid={`meeting-attention-${item.meetingId}`}
              role="status"
            />
          ) : null}
        </span>
      </SidebarMenuButton>
    </SidebarMenuItem>
  );
}

export function MeetingsSidebarSection({
  currentPubkey,
  items,
  selectedMeetingId,
  unreadMeetingIds,
  onSelectMeeting,
}: MeetingsSidebarSectionProps) {
  const [collapsed, setCollapsed] = React.useState(false);
  const [historyOpen, setHistoryOpen] = React.useState(false);
  const [activeLimit, setActiveLimit] = React.useState(ACTIVE_PAGE_SIZE);
  const [historyLimit, setHistoryLimit] = React.useState(HISTORY_PAGE_SIZE);
  const activeItems = React.useMemo(
    () =>
      items
        .filter(
          (item) => item.lifecycle !== "closed" && item.lifecycle !== "aborted",
        )
        .sort((left, right) => (right.updatedAt ?? 0) - (left.updatedAt ?? 0)),
    [items],
  );
  const historyItems = React.useMemo(
    () =>
      items
        .filter(
          (item) => item.lifecycle === "closed" || item.lifecycle === "aborted",
        )
        .sort((left, right) => (right.endedAt ?? 0) - (left.endedAt ?? 0)),
    [items],
  );
  const visibleActiveItems = activeItems.slice(0, activeLimit);
  const visibleHistoryItems = historyItems.slice(0, historyLimit);

  return (
    <>
      <SidebarGroup
        className="group/sidebar-section py-0"
        data-testid="meetings-section"
      >
        <SidebarGroupLabel className="relative pr-1">
          <button
            className="group/meeting-label flex min-w-0 items-center gap-1 text-left"
            onClick={() => setCollapsed((value) => !value)}
            type="button"
          >
            <span>Meetings</span>
            <span className="relative size-3">
              <ChevronDown
                className={cn(
                  "absolute inset-0 size-3 opacity-0 transition-[transform,opacity] group-hover/meeting-label:opacity-100 group-focus-visible/meeting-label:opacity-100",
                  collapsed && "-rotate-90",
                )}
              />
            </span>
          </button>
          <div className="ml-auto flex items-center gap-0.5">
            <button
              aria-label="Start a Meeting"
              className="flex size-6 items-center justify-center rounded-md text-sidebar-foreground/55 hover:bg-sidebar-accent hover:text-sidebar-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-sidebar-ring"
              data-testid="meeting-create-trigger"
              onClick={() => requestOpenCreateMeeting()}
              title="Start a Meeting"
              type="button"
            >
              <Plus className="size-4" />
            </button>
            <button
              aria-label="Meeting history"
              className="flex size-6 items-center justify-center rounded-md text-sidebar-foreground/55 hover:bg-sidebar-accent hover:text-sidebar-foreground focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-sidebar-ring"
              data-testid="meeting-history-trigger"
              onClick={() => setHistoryOpen(true)}
              title="Meeting history"
              type="button"
            >
              <History className="size-4" />
            </button>
          </div>
        </SidebarGroupLabel>
        {!collapsed ? (
          <SidebarGroupContent>
            <SidebarMenu data-testid="meeting-active-list">
              {visibleActiveItems.map((item) => (
                <MeetingRow
                  isSelected={item.meetingId === selectedMeetingId}
                  isUnread={unreadMeetingIds.has(item.meetingId)}
                  item={item}
                  key={item.meetingId}
                  needsAttention={
                    Boolean(currentPubkey) &&
                    item.humanFloorAttentionPubkey === currentPubkey
                  }
                  onSelect={() => onSelectMeeting(item.meetingId)}
                />
              ))}
              {activeItems.length === 0 ? (
                <SidebarMenuItem>
                  <span className="block px-2 py-1 text-xs text-sidebar-foreground/55 group-data-[collapsible=icon]:hidden">
                    No active meetings
                  </span>
                </SidebarMenuItem>
              ) : null}
              {activeItems.length > activeLimit ? (
                <SidebarMenuItem>
                  <Button
                    className="h-8 w-full justify-start text-xs"
                    onClick={() =>
                      setActiveLimit((current) => current + ACTIVE_PAGE_SIZE)
                    }
                    variant="ghost"
                  >
                    Show{" "}
                    {Math.min(
                      ACTIVE_PAGE_SIZE,
                      activeItems.length - activeLimit,
                    )}{" "}
                    more meetings
                  </Button>
                </SidebarMenuItem>
              ) : null}
            </SidebarMenu>
          </SidebarGroupContent>
        ) : null}
      </SidebarGroup>

      <Dialog onOpenChange={setHistoryOpen} open={historyOpen}>
        <DialogContent className="max-h-[80vh] max-w-lg overflow-hidden">
          <DialogHeader>
            <DialogTitle>Meeting history</DialogTitle>
            <DialogDescription>
              Meetings closed or aborted in this Community.
            </DialogDescription>
          </DialogHeader>
          <div
            className="min-h-0 space-y-2 overflow-y-auto"
            data-testid="meeting-history-list"
          >
            {historyItems.length === 0 ? (
              <div className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
                No completed meetings yet.
              </div>
            ) : (
              visibleHistoryItems.map((item) => (
                <Button
                  className="h-auto w-full justify-start gap-3 px-3 py-2 text-left"
                  key={item.meetingId}
                  onClick={() => {
                    setHistoryOpen(false);
                    onSelectMeeting(item.meetingId);
                  }}
                  variant="ghost"
                >
                  <CalendarClock className="size-4 shrink-0" />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-sm font-medium">
                      {item.title}
                    </span>
                    <span className="block text-xs text-muted-foreground">
                      {lifecycleLabel(item.lifecycle)}
                      {item.endedAt
                        ? ` · ${new Date(item.endedAt * 1_000).toLocaleString()}`
                        : ""}
                    </span>
                  </span>
                </Button>
              ))
            )}
            {historyItems.length > historyLimit ? (
              <Button
                className="w-full"
                onClick={() =>
                  setHistoryLimit((current) => current + HISTORY_PAGE_SIZE)
                }
                variant="outline"
              >
                Load{" "}
                {Math.min(
                  HISTORY_PAGE_SIZE,
                  historyItems.length - historyLimit,
                )}{" "}
                older meetings
              </Button>
            ) : null}
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
