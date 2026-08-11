import { Archive, ChevronDown, CloudOff, Plus, Search } from "lucide-react";
import * as React from "react";

import type { ProjectContextCoordinateOption } from "@/features/project-context/queryModel";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";

export type ProjectContextPickerSourceState =
  | "loading"
  | "ready"
  | "unavailable";

function optionSearchText(option: ProjectContextCoordinateOption) {
  return [
    option.title,
    option.typeLabel,
    option.state,
    option.status,
    option.description,
    option.searchTerms,
    option.coordinateKey,
  ]
    .filter(Boolean)
    .join(" ")
    .toLocaleLowerCase();
}

/** Searchable Project Coordinate picker shared by structural and semantic queries. */
export function ProjectContextCoordinatePicker({
  buttonLabel = "Add Coordinate",
  closeOnSelect,
  disabled,
  documentsState,
  meetingsState,
  onSelect,
  options,
  pickerTestId = "project-context-coordinate-picker",
  projectViewState,
  searchLabel = "Search Project Context Coordinates",
  searchTestId = "project-context-coordinate-search",
  selectedKeys,
}: {
  buttonLabel?: string;
  closeOnSelect: boolean;
  disabled: boolean;
  documentsState: ProjectContextPickerSourceState;
  meetingsState: ProjectContextPickerSourceState;
  onSelect: (option: ProjectContextCoordinateOption) => void;
  options: ProjectContextCoordinateOption[];
  pickerTestId?: string;
  projectViewState: ProjectContextPickerSourceState;
  searchLabel?: string;
  searchTestId?: string;
  selectedKeys: ReadonlySet<string>;
}) {
  const listboxId = React.useId();
  const [open, setOpen] = React.useState(false);
  const [search, setSearch] = React.useState("");
  const [highlightedIndex, setHighlightedIndex] = React.useState(0);
  const filtered = React.useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return options.filter(
      (option) =>
        !selectedKeys.has(option.coordinateKey) &&
        (!query || optionSearchText(option).includes(query)),
    );
  }, [options, search, selectedKeys]);
  const filteredIndexByKey = React.useMemo(
    () =>
      new Map(
        filtered.map((option, index) => [option.coordinateKey, index] as const),
      ),
    [filtered],
  );
  const activeOptionId = filtered[highlightedIndex]
    ? `${listboxId}-option-${highlightedIndex}`
    : undefined;

  React.useEffect(() => {
    if (filtered.length === 0) {
      setHighlightedIndex(0);
      return;
    }
    setHighlightedIndex((current) => Math.min(current, filtered.length - 1));
  }, [filtered.length]);

  React.useEffect(() => {
    if (!disabled) return;
    setOpen(false);
    setSearch("");
    setHighlightedIndex(0);
  }, [disabled]);

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) {
      setSearch("");
      setHighlightedIndex(0);
    }
  }

  function selectOption(option: ProjectContextCoordinateOption) {
    if (disabled || selectedKeys.has(option.coordinateKey)) return;
    if (closeOnSelect) handleOpenChange(false);
    onSelect(option);
    if (!closeOnSelect) {
      setSearch("");
      setHighlightedIndex(0);
    }
  }

  function moveHighlight(direction: 1 | -1) {
    if (filtered.length === 0) return;
    setHighlightedIndex(
      (current) => (current + direction + filtered.length) % filtered.length,
    );
  }

  function handleKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      moveHighlight(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveHighlight(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const option = filtered[highlightedIndex];
      if (option) selectOption(option);
    } else if (event.key === "Escape") {
      event.preventDefault();
      handleOpenChange(false);
    }
  }

  return (
    <Popover onOpenChange={handleOpenChange} open={open}>
      <PopoverTrigger asChild>
        <Button
          className="justify-between"
          data-testid={pickerTestId}
          disabled={disabled}
          size="sm"
          type="button"
          variant="outline"
        >
          <Plus />
          {buttonLabel}
          <ChevronDown className="ml-1 h-3.5 w-3.5" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-[min(30rem,var(--radix-popover-content-available-width))] overflow-hidden p-0"
        onOpenAutoFocus={(event) => event.preventDefault()}
      >
        <div className="flex items-center gap-2 border-b border-border/70 px-3 py-2">
          <Search className="h-4 w-4 shrink-0 text-muted-foreground" />
          <input
            aria-activedescendant={activeOptionId}
            aria-autocomplete="list"
            aria-controls={listboxId}
            aria-expanded={open}
            aria-label={searchLabel}
            autoCapitalize="none"
            autoComplete="off"
            autoCorrect="off"
            className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
            data-testid={searchTestId}
            onChange={(event) => {
              setSearch(event.target.value);
              setHighlightedIndex(0);
            }}
            onKeyDown={handleKeyDown}
            placeholder="Search title, type, status, or ID…"
            ref={(element) => element?.focus()}
            role="combobox"
            spellCheck={false}
            value={search}
          />
        </div>
        <div
          aria-label="Project Coordinates"
          className="max-h-80 overflow-y-auto overscroll-contain p-1"
          id={listboxId}
          role="listbox"
        >
          {(["project_view", "documents", "meetings"] as const).map((group) => {
            const groupOptions = filtered.filter(
              (option) => option.group === group,
            );
            const sourceState =
              group === "project_view"
                ? projectViewState
                : group === "documents"
                  ? documentsState
                  : meetingsState;
            const label =
              group === "project_view"
                ? "Project View"
                : group === "documents"
                  ? "Documents"
                  : "Meetings";
            if (groupOptions.length === 0 && sourceState === "ready") {
              return null;
            }
            return (
              <fieldset className="py-1" key={group}>
                <legend className="sr-only">{label}</legend>
                <div
                  aria-hidden="true"
                  className="flex items-center justify-between px-2 py-1 text-2xs font-semibold uppercase tracking-wider text-muted-foreground"
                >
                  <span>{label}</span>
                  {sourceState !== "ready" ? (
                    <span className="normal-case tracking-normal">
                      {sourceState === "loading" ? "Loading…" : "Unavailable"}
                    </span>
                  ) : null}
                </div>
                {groupOptions.map((option) => {
                  const index =
                    filteredIndexByKey.get(option.coordinateKey) ?? 0;
                  return (
                    <button
                      aria-selected={index === highlightedIndex}
                      className={cn(
                        "flex w-full items-start gap-2 rounded-lg px-2 py-2 text-left transition-colors hover:bg-muted/55",
                        index === highlightedIndex && "bg-muted/55",
                      )}
                      data-coordinate-key={option.coordinateKey}
                      id={`${listboxId}-option-${index}`}
                      key={option.coordinateKey}
                      onClick={() => selectOption(option)}
                      onMouseEnter={() => setHighlightedIndex(index)}
                      role="option"
                      type="button"
                    >
                      <span className="min-w-0 flex-1">
                        <span className="flex items-center gap-1.5">
                          <span className="truncate text-sm font-medium">
                            {option.title}
                          </span>
                          {option.state === "tombstoned" ? (
                            <Archive className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          ) : option.state === "unavailable" ? (
                            <CloudOff className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          ) : null}
                        </span>
                        <span className="mt-0.5 block truncate text-xs text-muted-foreground">
                          {option.typeLabel}
                          {option.state !== "active"
                            ? ` · ${
                                option.state === "terminal"
                                  ? "Terminal"
                                  : option.state === "tombstoned"
                                    ? "Tombstoned"
                                    : "Unavailable"
                              }`
                            : ""}
                          {option.status ? ` · ${option.status}` : ""}
                          {option.description ? ` · ${option.description}` : ""}
                        </span>
                        <span className="mt-0.5 block truncate font-mono text-2xs text-muted-foreground">
                          {option.coordinateKey}
                        </span>
                      </span>
                    </button>
                  );
                })}
              </fieldset>
            );
          })}
          {filtered.length === 0 &&
          projectViewState !== "loading" &&
          documentsState !== "loading" &&
          meetingsState !== "loading" ? (
            <p className="px-3 py-6 text-center text-sm text-muted-foreground">
              No current-project Coordinates match.
            </p>
          ) : null}
        </div>
      </PopoverContent>
    </Popover>
  );
}
