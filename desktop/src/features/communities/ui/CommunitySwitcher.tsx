import {
  ChevronDown,
  ChevronRight,
  Link2,
  Ticket,
  WifiOff,
} from "lucide-react";
import * as React from "react";

import type { Community } from "@/features/communities/types";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/shared/ui/dropdown-menu";
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/shared/ui/sidebar";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/shared/ui/tooltip";
import { cn } from "@/shared/lib/cn";
import type { ConnectionState } from "@/shared/api/relayClientShared";
import {
  isRelayConnectionDegraded,
  useRelayConnection,
} from "@/shared/api/useRelayConnection";
import { writeTextToClipboard } from "@/shared/lib/clipboard";
import { useActiveCommunityIcon } from "@/features/communities/useCommunityIcons";

const CONNECTION_STATE_LABEL: Record<ConnectionState, string> = {
  idle: "Not connected",
  connecting: "Connecting…",
  connected: "Connected",
  reconnecting: "Reconnecting to relay…",
  stalled: "Connection lost — relay is not responding",
  disconnected: "Disconnected from relay",
};

type CommunitySwitcherProps = {
  activeCommunity: Community | null;
  variant?: "sidebar" | "profile" | "profile-menu";
  canInvite?: boolean;
  onInvite?: () => void;
};

export function CommunityEmojiIcon({
  className,
  iconUrl,
}: {
  className: string;
  iconUrl?: string | null;
}) {
  if (iconUrl) {
    return (
      <span
        aria-hidden="true"
        className={cn(className, "h-5 overflow-hidden rounded-md")}
      >
        <img
          alt=""
          className="h-full w-full object-cover"
          draggable={false}
          src={iconUrl}
        />
      </span>
    );
  }
  return (
    <span aria-hidden="true" className={className}>
      <span className="-translate-y-px leading-normal">🐝</span>
    </span>
  );
}

export function CommunitySwitcher({
  activeCommunity,
  variant = "sidebar",
  canInvite = false,
  onInvite,
}: CommunitySwitcherProps) {
  const [dropdownOpen, setDropdownOpen] = React.useState(false);
  const profileMenuHoverTimer = React.useRef<number | null>(null);
  const connectionState = useRelayConnection();
  const degraded = isRelayConnectionDegraded(connectionState);
  const connectionLabel = CONNECTION_STATE_LABEL[connectionState];
  const activeIconQuery = useActiveCommunityIcon(activeCommunity?.relayUrl);
  const activeIcon = activeIconQuery.data ?? null;
  const isProfileVariant = variant === "profile";

  function clearProfileMenuHoverTimer() {
    if (profileMenuHoverTimer.current !== null) {
      window.clearTimeout(profileMenuHoverTimer.current);
      profileMenuHoverTimer.current = null;
    }
  }

  function scheduleProfileMenu(nextOpen: boolean) {
    if (variant !== "profile-menu") return;
    clearProfileMenuHoverTimer();
    profileMenuHoverTimer.current = window.setTimeout(
      () => setDropdownOpen(nextOpen),
      nextOpen ? 80 : 160,
    );
  }

  function handleProfileMenuOpenChange(nextOpen: boolean) {
    if (variant !== "profile-menu") {
      setDropdownOpen(nextOpen);
      return;
    }
    if (!nextOpen) {
      clearProfileMenuHoverTimer();
    }
    setDropdownOpen(nextOpen);
  }

  React.useEffect(
    () => () => {
      if (profileMenuHoverTimer.current !== null) {
        window.clearTimeout(profileMenuHoverTimer.current);
      }
    },
    [],
  );

  const triggerContent = (
    <>
      {degraded ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <span
              aria-hidden="false"
              className={
                isProfileVariant
                  ? "flex h-5 w-5 shrink-0 animate-pulse items-center justify-center rounded-md border border-sidebar-border/70 bg-sidebar-accent/40 text-destructive"
                  : "flex h-5 w-5 shrink-0 animate-pulse items-center justify-center text-destructive"
              }
              data-testid="relay-connection-warning"
              role="img"
            >
              <WifiOff className={isProfileVariant ? "h-4 w-4" : "h-4 w-4"} />
            </span>
          </TooltipTrigger>
          <TooltipContent side={isProfileVariant ? "top" : "bottom"}>
            {connectionLabel}
          </TooltipContent>
        </Tooltip>
      ) : (
        <CommunityEmojiIcon
          className={
            isProfileVariant
              ? "flex w-5 shrink-0 items-center justify-center rounded-md border border-sidebar-border/70 bg-sidebar-accent/40 text-2xs"
              : "flex w-5 shrink-0 items-center justify-center text-xs"
          }
          iconUrl={activeIcon}
        />
      )}
      <span
        className={
          degraded
            ? "min-w-0 flex-1 truncate font-medium text-destructive animate-pulse"
            : "min-w-0 flex-1 truncate font-medium"
        }
      >
        {activeCommunity?.name ?? "No community"}
      </span>
      {variant === "profile-menu" ? (
        <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
      ) : (
        <ChevronDown
          className={
            isProfileVariant
              ? "h-4 w-4 shrink-0 text-sidebar-foreground/45"
              : "h-4 w-4 shrink-0 text-sidebar-foreground/50"
          }
        />
      )}
    </>
  );

  const profileMenuPopover =
    variant === "profile-menu" ? (
      <Popover open={dropdownOpen} onOpenChange={handleProfileMenuOpenChange}>
        <PopoverTrigger asChild>
          <button
            aria-expanded={dropdownOpen}
            aria-haspopup="menu"
            aria-label={
              degraded
                ? `${activeCommunity?.name ?? "Community"} — ${connectionLabel}`
                : "Community actions"
            }
            className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-left text-sm text-popover-foreground outline-hidden transition-colors hover:bg-muted/50 focus:bg-muted/50 focus:outline-none focus-visible:bg-muted/50 focus-visible:outline-none data-[state=open]:bg-muted/50 data-[state=open]:text-popover-foreground"
            data-testid="community-switcher"
            onMouseEnter={() => scheduleProfileMenu(true)}
            onMouseLeave={() => scheduleProfileMenu(false)}
            role="menuitem"
            type="button"
          >
            {triggerContent}
          </button>
        </PopoverTrigger>
        <PopoverContent
          align="end"
          className="w-60 p-1"
          onMouseEnter={() => scheduleProfileMenu(true)}
          onMouseLeave={() => scheduleProfileMenu(false)}
          onOpenAutoFocus={(event) => event.preventDefault()}
          side="right"
          sideOffset={0}
        >
          <div
            aria-label="Community actions"
            data-testid="profile-community-actions"
            role="menu"
          >
            {activeCommunity ? (
              <>
                <button
                  className="flex min-h-9 w-full items-center gap-2 rounded-lg px-3 py-2 text-left text-sm outline-hidden transition-colors hover:bg-muted/50 focus:bg-muted/50 focus:outline-none focus-visible:bg-muted/50 focus-visible:outline-none"
                  onClick={() => {
                    setDropdownOpen(false);
                    void writeTextToClipboard(activeCommunity.relayUrl);
                  }}
                  role="menuitem"
                  type="button"
                >
                  <Link2 className="h-4 w-4" />
                  <span>Copy local Relay URL</span>
                </button>
                {canInvite && onInvite ? (
                  <button
                    className="flex min-h-9 w-full items-center gap-2 rounded-lg px-3 py-2 text-left text-sm outline-hidden transition-colors hover:bg-muted/50 focus:bg-muted/50 focus:outline-none focus-visible:bg-muted/50 focus-visible:outline-none"
                    onClick={() => {
                      setDropdownOpen(false);
                      onInvite();
                    }}
                    role="menuitem"
                    type="button"
                  >
                    <Ticket className="h-4 w-4" />
                    <span>Invite to community</span>
                  </button>
                ) : null}
              </>
            ) : null}
          </div>
        </PopoverContent>
      </Popover>
    ) : null;

  const switcherDropdown = (
    <DropdownMenu
      modal={false}
      open={dropdownOpen}
      onOpenChange={setDropdownOpen}
    >
      <DropdownMenuTrigger asChild>
        {variant === "profile" ? (
          <button
            aria-label={
              degraded
                ? `${activeCommunity?.name ?? "Community"} — ${connectionLabel}`
                : "Local community actions"
            }
            className="flex min-w-0 max-w-full items-center gap-1.5 rounded-md py-0.5 text-left text-xs text-sidebar-foreground/50 outline-hidden transition-colors hover:text-sidebar-foreground focus:outline-none focus-visible:outline-none data-[state=open]:text-sidebar-foreground"
            data-testid="community-switcher"
            type="button"
          >
            {triggerContent}
          </button>
        ) : (
          <SidebarMenuButton
            aria-label={
              degraded
                ? `${activeCommunity?.name ?? "Community"} — ${connectionLabel}`
                : undefined
            }
            className="h-auto gap-2 rounded-xl px-2.5 py-2 data-[state=open]:bg-sidebar-accent"
            data-testid="community-switcher"
            type="button"
          >
            {triggerContent}
          </SidebarMenuButton>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="w-(--radix-dropdown-menu-trigger-width) min-w-[220px]"
        onCloseAutoFocus={(e) => e.preventDefault()}
        side={variant === "profile" ? "top" : "bottom"}
        sideOffset={4}
      >
        {activeCommunity ? (
          <DropdownMenuItem
            onSelect={() => void writeTextToClipboard(activeCommunity.relayUrl)}
          >
            <Link2 className="h-4 w-4" />
            <span>Copy local Relay URL</span>
          </DropdownMenuItem>
        ) : null}
        {canInvite && onInvite ? (
          <DropdownMenuItem onSelect={onInvite}>
            <Ticket className="h-4 w-4" />
            <span>Invite to local community</span>
          </DropdownMenuItem>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );

  return variant === "profile" ? (
    switcherDropdown
  ) : variant === "profile-menu" ? (
    profileMenuPopover
  ) : (
    <SidebarMenu>
      <SidebarMenuItem>{switcherDropdown}</SidebarMenuItem>
    </SidebarMenu>
  );
}
