import { Bot, Check, ChevronsUpDown, Search, UserRound } from "lucide-react";
import * as React from "react";

import {
  filterProjectRoleCandidates,
  normalizeRoleCandidateInput,
  type ProjectRoleCandidate,
} from "@/features/project-view/projectRoleCandidates";
import { cn } from "@/shared/lib/cn";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";
import { UserAvatar } from "@/shared/ui/UserAvatar";

type ProjectRoleCandidatePickerProps = {
  candidates: readonly ProjectRoleCandidate[];
  disabled?: boolean;
  id: string;
  isLoading: boolean;
  isPartial: boolean;
  onChange: (pubkey: string) => void;
  value: string;
};

function titleCase(value: string) {
  return `${value.charAt(0).toUpperCase()}${value.slice(1)}`;
}

function candidateIdentityDetails(candidate: ProjectRoleCandidate) {
  const details = [
    candidate.identityType === "agent"
      ? "Agent"
      : titleCase(candidate.communityRole ?? "member"),
  ];
  if (candidate.ownerPubkey) {
    details.push(
      `managed by ${
        candidate.managedByCurrentUser
          ? "you"
          : (candidate.ownerDisplayName ??
            truncatePubkey(candidate.ownerPubkey))
      }`,
    );
  }
  if (candidate.runtimeStatus) {
    details.push(titleCase(candidate.runtimeStatus));
  }
  details.push(truncatePubkey(candidate.pubkey));
  return details.join(" · ");
}

function candidateRoleStatus(candidate: ProjectRoleCandidate) {
  if (candidate.isCurrentAssignee) return "Current assignee";
  if (candidate.openProposal) {
    return `${titleCase(candidate.openProposal.proposalType)} pending`;
  }
  if (candidate.activeAssignment) {
    return `Assigned to ${candidate.activeAssignment.roleName}`;
  }
  return "Available";
}

function CandidateRow({
  candidate,
  highlighted,
  onHighlight,
  onSelect,
  selected,
}: {
  candidate: ProjectRoleCandidate;
  highlighted: boolean;
  onHighlight: () => void;
  onSelect: () => void;
  selected: boolean;
}) {
  const unavailable = candidate.isCurrentAssignee;
  const roleStatus = candidateRoleStatus(candidate);

  return (
    <button
      aria-disabled={unavailable}
      aria-label={`${candidate.displayName}, ${candidateIdentityDetails(candidate)}, ${roleStatus}`}
      aria-selected={selected}
      className={cn(
        "flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-muted/60 focus-visible:bg-muted/60 focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring",
        highlighted && "bg-muted/60",
        unavailable && "cursor-not-allowed opacity-60",
      )}
      data-testid={`project-role-candidate-option-${candidate.pubkey}`}
      id={`project-role-candidate-option-${candidate.pubkey}`}
      onClick={() => {
        if (!unavailable) onSelect();
      }}
      onMouseMove={onHighlight}
      role="option"
      tabIndex={-1}
      type="button"
    >
      <UserAvatar
        avatarUrl={candidate.avatarUrl}
        displayName={candidate.displayName}
        size="md"
      />
      <span className="min-w-0 flex-1">
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm font-medium">
            {candidate.displayName}
          </span>
          <span className="inline-flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
            {candidate.identityType === "agent" ? (
              <Bot aria-hidden="true" className="h-3 w-3" />
            ) : (
              <UserRound aria-hidden="true" className="h-3 w-3" />
            )}
            {candidate.identityType}
          </span>
        </span>
        <span className="block truncate text-xs text-muted-foreground">
          {candidateIdentityDetails(candidate)}
        </span>
        <span
          className={cn(
            "block truncate text-xs",
            candidate.isCurrentAssignee
              ? "text-muted-foreground"
              : "text-foreground/80",
          )}
        >
          {roleStatus}
        </span>
      </span>
      <Check
        aria-hidden="true"
        className={cn(
          "h-4 w-4 shrink-0",
          selected ? "opacity-100" : "opacity-0",
        )}
      />
    </button>
  );
}

/** Community-scoped, Human-readable selector for a Role offer candidate. */
export function ProjectRoleCandidatePicker({
  candidates,
  disabled,
  id,
  isLoading,
  isPartial,
  onChange,
  value,
}: ProjectRoleCandidatePickerProps) {
  const [open, setOpen] = React.useState(false);
  const [manualMode, setManualMode] = React.useState(false);
  const [query, setQuery] = React.useState("");
  const [highlightedIndex, setHighlightedIndex] = React.useState(0);
  const searchInputRef = React.useRef<HTMLInputElement>(null);
  const listboxId = `${id}-listbox`;
  const normalizedValue = normalizeRoleCandidateInput(value);
  const selected = normalizedValue
    ? candidates.find((candidate) => candidate.pubkey === normalizedValue)
    : undefined;
  const grouped = React.useMemo(
    () => filterProjectRoleCandidates(candidates, query),
    [candidates, query],
  );
  const visibleCandidates = React.useMemo(
    () => [...grouped.agents, ...grouped.people],
    [grouped.agents, grouped.people],
  );
  const indexesByPubkey = React.useMemo(
    () =>
      new Map(
        visibleCandidates.map((candidate, index) => [candidate.pubkey, index]),
      ),
    [visibleCandidates],
  );
  const manualInvalid = value.trim().length > 0 && normalizedValue === null;

  React.useEffect(() => {
    if (highlightedIndex >= visibleCandidates.length) {
      setHighlightedIndex(0);
    }
  }, [highlightedIndex, visibleCandidates.length]);

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) {
      setQuery("");
      setHighlightedIndex(0);
    }
  }

  function selectCandidate(candidate: ProjectRoleCandidate) {
    if (candidate.isCurrentAssignee) return;
    onChange(candidate.pubkey);
    setManualMode(false);
    handleOpenChange(false);
  }

  function handleSearchKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      handleOpenChange(false);
      return;
    }
    if (visibleCandidates.length === 0) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setHighlightedIndex((index) => (index + 1) % visibleCandidates.length);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setHighlightedIndex(
        (index) =>
          (index - 1 + visibleCandidates.length) % visibleCandidates.length,
      );
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      const candidate = visibleCandidates[highlightedIndex];
      if (candidate) selectCandidate(candidate);
    }
  }

  return (
    <div className="space-y-1.5 text-sm">
      <label className="font-medium" htmlFor={manualMode ? `${id}-manual` : id}>
        Candidate
      </label>
      {manualMode ? (
        <div className="space-y-1.5">
          <Input
            aria-invalid={manualInvalid}
            autoFocus
            data-testid="project-role-candidate"
            disabled={disabled}
            id={`${id}-manual`}
            onChange={(event) => onChange(event.target.value)}
            placeholder="64-character public key or npub"
            value={value}
          />
          {manualInvalid ? (
            <p className="text-xs text-destructive" role="alert">
              Enter a valid 64-character hex public key or npub.
            </p>
          ) : null}
          <Button
            disabled={disabled}
            onClick={() => setManualMode(false)}
            size="xs"
            type="button"
            variant="ghost"
          >
            Choose from Community directory
          </Button>
        </div>
      ) : (
        <Popover onOpenChange={handleOpenChange} open={open}>
          <PopoverTrigger asChild>
            <button
              aria-controls={listboxId}
              aria-expanded={open}
              className={cn(
                "flex min-h-10 w-full items-center justify-between gap-3 rounded-lg border border-input/40 bg-background px-3 py-2 text-left text-sm transition-colors focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50",
                !selected && "text-muted-foreground",
              )}
              data-testid="project-role-candidate-picker"
              disabled={disabled}
              id={id}
              role="combobox"
              type="button"
            >
              {selected ? (
                <span className="flex min-w-0 items-center gap-2">
                  <UserAvatar
                    avatarUrl={selected.avatarUrl}
                    displayName={selected.displayName}
                    size="sm"
                  />
                  <span className="min-w-0">
                    <span className="block truncate font-medium text-foreground">
                      {selected.displayName}
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {candidateIdentityDetails(selected)}
                    </span>
                  </span>
                </span>
              ) : (
                <span>Select a person or agent…</span>
              )}
              <ChevronsUpDown className="h-4 w-4 shrink-0" />
            </button>
          </PopoverTrigger>
          <PopoverContent
            align="start"
            className="w-[min(32rem,calc(100vw-3rem))] overflow-hidden p-0"
            onOpenAutoFocus={(event) => {
              event.preventDefault();
              searchInputRef.current?.focus();
            }}
          >
            <div className="flex items-center gap-2 border-b border-border px-3 py-2">
              <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
              <input
                aria-activedescendant={
                  visibleCandidates[highlightedIndex]
                    ? `project-role-candidate-option-${visibleCandidates[highlightedIndex].pubkey}`
                    : undefined
                }
                aria-controls={listboxId}
                aria-expanded={open}
                aria-label="Search people or agents"
                aria-autocomplete="list"
                autoCapitalize="none"
                autoComplete="off"
                autoCorrect="off"
                className="min-w-0 flex-1 bg-transparent text-sm outline-hidden placeholder:text-muted-foreground"
                data-testid="project-role-candidate-search"
                onChange={(event) => {
                  setQuery(event.target.value);
                  setHighlightedIndex(0);
                }}
                onKeyDown={handleSearchKeyDown}
                placeholder="Search people or agents…"
                ref={searchInputRef}
                role="combobox"
                spellCheck={false}
                value={query}
              />
            </div>
            <div
              className="max-h-80 overflow-y-auto p-1"
              id={listboxId}
              role="listbox"
            >
              {isLoading ? (
                <p
                  aria-live="polite"
                  className="px-3 py-4 text-center text-xs text-muted-foreground"
                  role="status"
                >
                  Loading Community identities…
                </p>
              ) : visibleCandidates.length === 0 ? (
                <p className="px-3 py-4 text-center text-xs text-muted-foreground">
                  {query.trim()
                    ? "No matching people or agents."
                    : "No eligible Community identities found."}
                </p>
              ) : (
                <>
                  {grouped.agents.length > 0 ? (
                    <div>
                      <div
                        className="px-3 pb-1 pt-2 text-2xs font-semibold uppercase tracking-wider text-muted-foreground"
                        id={`${listboxId}-agents`}
                      >
                        Agents
                      </div>
                      {grouped.agents.map((candidate) => (
                        <CandidateRow
                          candidate={candidate}
                          highlighted={
                            indexesByPubkey.get(candidate.pubkey) ===
                            highlightedIndex
                          }
                          key={candidate.pubkey}
                          onHighlight={() =>
                            setHighlightedIndex(
                              indexesByPubkey.get(candidate.pubkey) ?? 0,
                            )
                          }
                          onSelect={() => selectCandidate(candidate)}
                          selected={selected?.pubkey === candidate.pubkey}
                        />
                      ))}
                    </div>
                  ) : null}
                  {grouped.people.length > 0 ? (
                    <div>
                      <div
                        className="px-3 pb-1 pt-2 text-2xs font-semibold uppercase tracking-wider text-muted-foreground"
                        id={`${listboxId}-people`}
                      >
                        People
                      </div>
                      {grouped.people.map((candidate) => (
                        <CandidateRow
                          candidate={candidate}
                          highlighted={
                            indexesByPubkey.get(candidate.pubkey) ===
                            highlightedIndex
                          }
                          key={candidate.pubkey}
                          onHighlight={() =>
                            setHighlightedIndex(
                              indexesByPubkey.get(candidate.pubkey) ?? 0,
                            )
                          }
                          onSelect={() => selectCandidate(candidate)}
                          selected={selected?.pubkey === candidate.pubkey}
                        />
                      ))}
                    </div>
                  ) : null}
                </>
              )}
            </div>
            <div className="border-t border-border px-3 py-2">
              {isPartial ? (
                <p
                  className="mb-1.5 text-xs text-muted-foreground"
                  role="status"
                >
                  Some identities may not be synced yet.
                </p>
              ) : null}
              <Button
                data-testid="project-role-candidate-manual-toggle"
                disabled={disabled}
                onClick={() => {
                  handleOpenChange(false);
                  setManualMode(true);
                }}
                size="xs"
                type="button"
                variant="ghost"
              >
                Use a public key manually…
              </Button>
            </div>
          </PopoverContent>
        </Popover>
      )}
    </div>
  );
}
