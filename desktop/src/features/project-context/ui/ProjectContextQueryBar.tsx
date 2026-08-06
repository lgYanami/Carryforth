import { Archive, ChevronDown, CloudOff, Plus, Search, X } from "lucide-react";
import * as React from "react";

import {
  addProjectContextDraftCoordinate,
  changeProjectContextDraftMode,
  projectContextDraftFromQuery,
  projectContextDraftValidationMessage,
  projectContextQueryFromDraft,
  removeProjectContextDraftCoordinate,
  type ProjectContextCoordinateOption,
} from "@/features/project-context/queryModel";
import type { ProjectContextQueryMode } from "@/features/project-context/routeState";
import {
  projectContextCoordinateKey,
  projectContextQueryKey,
  type ProjectContextQuery,
} from "@/shared/api/tauriProjectContext";
import { cn } from "@/shared/lib/cn";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui/popover";

export type ProjectContextPickerSourceState =
  | "loading"
  | "ready"
  | "unavailable";

const MODES: Array<{ mode: ProjectContextQueryMode; label: string }> = [
  { mode: "all", label: "All Context" },
  { mode: "exact", label: "Exact" },
  { mode: "incident", label: "Incident" },
  { mode: "contains_all", label: "Contains all" },
];

function modeLabel(mode: ProjectContextQueryMode) {
  return MODES.find((candidate) => candidate.mode === mode)?.label ?? mode;
}

function optionSearchText(option: ProjectContextCoordinateOption) {
  return [
    option.title,
    option.typeLabel,
    option.status,
    option.description,
    option.coordinateKey,
  ]
    .filter(Boolean)
    .join(" ")
    .toLocaleLowerCase();
}

function CoordinatePicker({
  disabled,
  documentsState,
  onSelect,
  options,
  projectViewState,
  selectedKeys,
}: {
  disabled: boolean;
  documentsState: ProjectContextPickerSourceState;
  onSelect: (option: ProjectContextCoordinateOption) => void;
  options: ProjectContextCoordinateOption[];
  projectViewState: ProjectContextPickerSourceState;
  selectedKeys: ReadonlySet<string>;
}) {
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

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) {
      setSearch("");
      setHighlightedIndex(0);
    }
  }

  function selectOption(option: ProjectContextCoordinateOption) {
    if (selectedKeys.has(option.coordinateKey)) return;
    onSelect(option);
    setSearch("");
    setHighlightedIndex(0);
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
          aria-expanded={open}
          className="justify-between"
          data-testid="project-context-coordinate-picker"
          disabled={disabled}
          role="combobox"
          size="sm"
          type="button"
          variant="outline"
        >
          <Plus />
          Add Coordinate
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
            aria-label="Search Project Context Coordinates"
            autoCapitalize="none"
            autoComplete="off"
            autoCorrect="off"
            className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
            data-testid="project-context-coordinate-search"
            onChange={(event) => {
              setSearch(event.target.value);
              setHighlightedIndex(0);
            }}
            onKeyDown={handleKeyDown}
            placeholder="Search title, type, status, or ID…"
            ref={(element) => element?.focus()}
            spellCheck={false}
            value={search}
          />
        </div>
        <div
          className="max-h-80 overflow-y-auto overscroll-contain p-1"
          role="listbox"
        >
          {(["project_view", "documents"] as const).map((group) => {
            const groupOptions = filtered.filter(
              (option) => option.group === group,
            );
            const sourceState =
              group === "project_view" ? projectViewState : documentsState;
            const label =
              group === "project_view" ? "Project View" : "Documents";
            if (groupOptions.length === 0 && sourceState === "ready") {
              return null;
            }
            return (
              <section className="py-1" key={group}>
                <div className="flex items-center justify-between px-2 py-1 text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                  <span>{label}</span>
                  {sourceState !== "ready" ? (
                    <span className="normal-case tracking-normal">
                      {sourceState === "loading" ? "Loading…" : "Unavailable"}
                    </span>
                  ) : null}
                </div>
                {groupOptions.map((option) => {
                  const index = filtered.indexOf(option);
                  return (
                    <button
                      aria-selected={false}
                      className={cn(
                        "flex w-full items-start gap-2 rounded-lg px-2 py-2 text-left transition-colors hover:bg-muted/55",
                        index === highlightedIndex && "bg-muted/55",
                      )}
                      data-coordinate-key={option.coordinateKey}
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
              </section>
            );
          })}
          {filtered.length === 0 &&
          projectViewState !== "loading" &&
          documentsState !== "loading" ? (
            <p className="px-3 py-6 text-center text-sm text-muted-foreground">
              No current-project Coordinates match.
            </p>
          ) : null}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** Draft-first controls for the three domain query variants and All alias. */
export function ProjectContextQueryBar({
  appliedQuery,
  coordinateOptions,
  documentsState,
  onRun,
  projectViewState,
}: {
  appliedQuery: ProjectContextQuery;
  coordinateOptions: ProjectContextCoordinateOption[];
  documentsState: ProjectContextPickerSourceState;
  onRun: (query: ProjectContextQuery) => void;
  projectViewState: ProjectContextPickerSourceState;
}) {
  const appliedKey = projectContextQueryKey(appliedQuery);
  const lastAppliedKey = React.useRef(appliedKey);
  const [draft, setDraft] = React.useState(() =>
    projectContextDraftFromQuery(appliedQuery),
  );

  React.useEffect(() => {
    if (lastAppliedKey.current === appliedKey) return;
    lastAppliedKey.current = appliedKey;
    setDraft(projectContextDraftFromQuery(appliedQuery));
  }, [appliedKey, appliedQuery]);

  const selectedKeys = React.useMemo(
    () =>
      new Set(
        draft.coordinates.map((coordinate) =>
          projectContextCoordinateKey(coordinate),
        ),
      ),
    [draft.coordinates],
  );
  const optionByKey = React.useMemo(
    () =>
      new Map(
        coordinateOptions.map((option) => [option.coordinateKey, option]),
      ),
    [coordinateOptions],
  );
  const validation = projectContextDraftValidationMessage(draft);
  let draftKey: string | undefined;
  if (!validation) {
    draftKey = projectContextQueryKey(projectContextQueryFromDraft(draft));
  }
  const dirty = draftKey !== appliedKey;

  return (
    <section
      className="border-b border-border/70 bg-background/70 px-3 py-3 sm:px-5"
      data-draft-dirty={dirty}
      data-testid="project-context-query-bar"
    >
      <div className="mx-auto flex max-w-6xl flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <fieldset className="flex flex-wrap gap-1 rounded-xl border border-border/70 bg-muted/25 p-1">
            <legend className="sr-only">Project Context query mode</legend>
            {MODES.map((candidate) => (
              <Button
                aria-pressed={draft.mode === candidate.mode}
                data-testid={`project-context-mode-${candidate.mode}`}
                key={candidate.mode}
                onClick={() =>
                  setDraft((current) =>
                    changeProjectContextDraftMode(current, candidate.mode),
                  )
                }
                size="xs"
                type="button"
                variant={draft.mode === candidate.mode ? "secondary" : "ghost"}
              >
                {candidate.label}
              </Button>
            ))}
          </fieldset>
          <CoordinatePicker
            disabled={
              draft.mode === "all" ||
              (draft.mode === "incident" && draft.coordinates.length === 1)
            }
            documentsState={documentsState}
            onSelect={(option) =>
              setDraft((current) =>
                addProjectContextDraftCoordinate(current, option.coordinate),
              )
            }
            options={coordinateOptions}
            projectViewState={projectViewState}
            selectedKeys={selectedKeys}
          />
          {draft.coordinates.length > 0 ? (
            <Button
              data-testid="project-context-clear-coordinates"
              onClick={() =>
                setDraft((current) => ({ ...current, coordinates: [] }))
              }
              size="sm"
              type="button"
              variant="ghost"
            >
              Clear
            </Button>
          ) : null}
          <div className="min-w-4 flex-1" />
          <Badge variant={dirty ? "warning" : "outline"}>
            {dirty
              ? "Draft · not applied"
              : `Applied · ${modeLabel(draft.mode)}`}
          </Badge>
          <Button
            data-testid="project-context-run-query"
            disabled={Boolean(validation) || !dirty}
            onClick={() => onRun(projectContextQueryFromDraft(draft))}
            size="sm"
            type="button"
          >
            Run
          </Button>
        </div>

        {draft.coordinates.length > 0 ? (
          <ul
            aria-label="Query Coordinates"
            className="flex list-none flex-wrap gap-1.5 p-0"
            data-testid="project-context-query-chips"
          >
            {draft.coordinates.map((coordinate) => {
              const key = projectContextCoordinateKey(coordinate);
              const option = optionByKey.get(key);
              return (
                <li
                  className="inline-flex max-w-full items-center gap-1 rounded-full border border-border/70 bg-card px-2 py-1 text-xs"
                  data-coordinate-key={key}
                  key={key}
                >
                  <span className="max-w-56 truncate font-medium">
                    {option?.title ?? key}
                  </span>
                  <span className="text-muted-foreground">
                    {option?.typeLabel ?? "Coordinate"}
                  </span>
                  <button
                    aria-label={`Remove ${option?.title ?? key}`}
                    className="rounded-full p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    onClick={() =>
                      setDraft((current) =>
                        removeProjectContextDraftCoordinate(current, key),
                      )
                    }
                    type="button"
                  >
                    <X className="h-3 w-3" />
                  </button>
                </li>
              );
            })}
          </ul>
        ) : null}

        <p
          className={cn(
            "text-xs",
            validation
              ? "text-amber-700 dark:text-amber-300"
              : "text-muted-foreground",
          )}
          data-testid="project-context-query-guidance"
          role={validation ? "status" : undefined}
        >
          {validation ??
            (draft.mode === "all"
              ? "All Context is the complete contains-all empty-set query."
              : "Editing this draft does not change the graph until you Run it.")}
        </p>
      </div>
    </section>
  );
}
