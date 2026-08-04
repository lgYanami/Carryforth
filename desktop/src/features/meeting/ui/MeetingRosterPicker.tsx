import { Bot, Check, Search, UserRound } from "lucide-react";
import type { UIEventHandler } from "react";

import type { MeetingAgentCapability } from "@/features/meeting/createMeetingModel";
import { formatOwnerLabel } from "@/features/profile/lib/identity";
import type { UserProfileLookup } from "@/features/profile/lib/identity";
import { ProfileAvatar } from "@/features/profile/ui/ProfileAvatar";
import { SelectedRecipientChip } from "@/features/profile/ui/SelectedRecipientChip";
import type { UserSearchResult } from "@/shared/api/types";
import { cn } from "@/shared/lib/cn";
import { Input } from "@/shared/ui/input";

import { formatRecipientName } from "@/features/messages/ui/useNewMessageRecipients";

export type MeetingRosterDisplayCandidate = UserSearchResult & {
  actionCapability: MeetingAgentCapability;
};

function capabilityLabel(capability: MeetingAgentCapability): string {
  switch (capability) {
    case "compatible":
      return "Action ready";
    case "incompatible":
      return "Missing action capability";
    case "unknown":
      return "Capability unknown";
    default:
      return "Human";
  }
}

function capabilityClass(capability: MeetingAgentCapability): string {
  switch (capability) {
    case "compatible":
      return "text-emerald-600 dark:text-emerald-400";
    case "incompatible":
      return "text-destructive";
    case "unknown":
      return "text-amber-600 dark:text-amber-400";
    default:
      return "text-muted-foreground";
  }
}

export function MeetingRosterPicker({
  candidates,
  currentHost,
  currentPubkey,
  disabled,
  hasReachedLimit,
  isLoading,
  onDirectoryScroll,
  onRemove,
  onSearchChange,
  onSelect,
  ownerProfiles,
  searchError,
  searchQuery,
  selected,
}: {
  candidates: MeetingRosterDisplayCandidate[];
  currentHost: { displayName: string; avatarUrl: string | null };
  currentPubkey: string;
  disabled: boolean;
  hasReachedLimit: boolean;
  isLoading: boolean;
  onDirectoryScroll: UIEventHandler<HTMLDivElement>;
  onRemove: (pubkey: string) => void;
  onSearchChange: (value: string) => void;
  onSelect: (candidate: UserSearchResult) => void;
  ownerProfiles?: UserProfileLookup;
  searchError: Error | null;
  searchQuery: string;
  selected: MeetingRosterDisplayCandidate[];
}) {
  return (
    <div className="space-y-3" data-testid="meeting-roster-picker">
      <div className="rounded-lg border bg-muted/20 p-3">
        <div className="flex items-center gap-3" data-testid="meeting-host-row">
          <ProfileAvatar
            avatarUrl={currentHost.avatarUrl}
            className="size-8 text-xs shadow-none"
            iconClassName="size-4"
            label={currentHost.displayName}
          />
          <div className="min-w-0 flex-1">
            <p className="truncate text-sm font-medium">
              {currentHost.displayName}
            </p>
            <p className="text-xs text-muted-foreground">
              You · Human host · always included
            </p>
          </div>
          <Check className="size-4 text-emerald-600 dark:text-emerald-400" />
        </div>
      </div>

      {selected.length > 0 ? (
        <div
          className="flex flex-wrap gap-2"
          data-testid="meeting-roster-selected"
        >
          {selected.map((candidate) => (
            <div className="flex items-center gap-1" key={candidate.pubkey}>
              <SelectedRecipientChip
                disabled={disabled}
                label={formatRecipientName(candidate)}
                onRemove={() => onRemove(candidate.pubkey)}
                testIds={{
                  chip: `meeting-selected-${candidate.pubkey}`,
                  name: `meeting-selected-name-${candidate.pubkey}`,
                  pubkey: `meeting-selected-pubkey-${candidate.pubkey}`,
                }}
                user={candidate}
              />
              {candidate.isAgent ? (
                <span
                  className={cn(
                    "text-xs",
                    capabilityClass(candidate.actionCapability),
                  )}
                >
                  {capabilityLabel(candidate.actionCapability)}
                </span>
              ) : null}
            </div>
          ))}
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">
          Choose at least one other participant.
        </p>
      )}

      <div className="relative">
        <Search className="pointer-events-none absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          aria-label="Search Meeting participants"
          autoComplete="off"
          className="pl-9"
          data-testid="meeting-roster-search"
          disabled={disabled}
          onChange={(event) => onSearchChange(event.target.value)}
          placeholder="Search people and agents"
          value={searchQuery}
        />
      </div>

      <div
        className="max-h-52 overflow-y-auto rounded-lg border"
        data-testid="meeting-roster-results"
        onScroll={onDirectoryScroll}
      >
        {isLoading ? (
          <p className="px-3 py-4 text-sm text-muted-foreground">
            Loading people and agents…
          </p>
        ) : searchError ? (
          <p className="px-3 py-4 text-sm text-destructive">
            Participant directory unavailable: {searchError.message}
          </p>
        ) : candidates.length === 0 ? (
          <p className="px-3 py-4 text-sm text-muted-foreground">
            No matching people or agents.
          </p>
        ) : (
          candidates.map((candidate) => {
            const ownerLabel = formatOwnerLabel(
              candidate.ownerPubkey,
              currentPubkey,
              ownerProfiles,
            );
            return (
              <button
                className="flex min-h-14 w-full items-center gap-3 border-b px-3 py-2 text-left last:border-b-0 hover:bg-muted/50 focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                data-testid={`meeting-roster-candidate-${candidate.pubkey}`}
                disabled={disabled || hasReachedLimit}
                key={candidate.pubkey}
                onClick={() => onSelect(candidate)}
                type="button"
              >
                <ProfileAvatar
                  avatarUrl={candidate.avatarUrl}
                  className="size-8 text-xs shadow-none"
                  iconClassName="size-4"
                  label={formatRecipientName(candidate)}
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm font-medium">
                    {formatRecipientName(candidate)}
                  </span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {candidate.isAgent
                      ? ownerLabel
                        ? `Agent · managed by ${ownerLabel}`
                        : "Agent"
                      : "Human"}
                  </span>
                </span>
                <span
                  className={cn(
                    "inline-flex shrink-0 items-center gap-1 text-xs",
                    capabilityClass(candidate.actionCapability),
                  )}
                >
                  {candidate.isAgent ? (
                    <Bot className="size-3" />
                  ) : (
                    <UserRound className="size-3" />
                  )}
                  {capabilityLabel(candidate.actionCapability)}
                </span>
              </button>
            );
          })
        )}
      </div>
    </div>
  );
}
