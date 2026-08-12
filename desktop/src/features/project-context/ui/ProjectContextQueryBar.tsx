import { X } from "lucide-react";
import * as React from "react";

import {
  addProjectContextDraftCoordinate,
  changeProjectContextDraftMode,
  projectContextDraftValidationMessage,
  projectContextQueryFromDraft,
  removeProjectContextDraftCoordinate,
  type ProjectContextCoordinateOption,
  type ProjectContextQueryDraft,
} from "@/features/project-context/queryModel";
import type { ProjectContextQueryMode } from "@/features/project-context/routeState";
import {
  ProjectContextCoordinatePicker,
  type ProjectContextPickerSourceState,
} from "@/features/project-context/ui/ProjectContextCoordinatePicker";
import {
  projectContextCoordinateKey,
  projectContextQueryKey,
  type ProjectContextQuery,
} from "@/shared/api/tauriProjectContext";
import { cn } from "@/shared/lib/cn";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

export type { ProjectContextPickerSourceState } from "@/features/project-context/ui/ProjectContextCoordinatePicker";

const MODES: Array<{ mode: ProjectContextQueryMode; label: string }> = [
  { mode: "all", label: "All Context" },
  { mode: "exact", label: "Exact" },
  { mode: "incident", label: "Incident" },
  { mode: "contains_all", label: "Contains all" },
];

function modeLabel(mode: ProjectContextQueryMode) {
  return MODES.find((candidate) => candidate.mode === mode)?.label ?? mode;
}

/** Draft-first controls for the three domain query variants and All alias. */
export function ProjectContextQueryBar({
  appliedQuery,
  coordinateOptions,
  documentsState,
  draft,
  meetingsState,
  onDraftChange,
  onRun,
  panel = false,
  projectViewState,
  runDisabled = false,
  runDisabledReason,
}: {
  appliedQuery: ProjectContextQuery;
  coordinateOptions: ProjectContextCoordinateOption[];
  documentsState: ProjectContextPickerSourceState;
  draft: ProjectContextQueryDraft;
  meetingsState: ProjectContextPickerSourceState;
  onDraftChange: React.Dispatch<React.SetStateAction<ProjectContextQueryDraft>>;
  onRun: (query: ProjectContextQuery) => void;
  panel?: boolean;
  projectViewState: ProjectContextPickerSourceState;
  runDisabled?: boolean;
  runDisabledReason?: string;
}) {
  const appliedKey = projectContextQueryKey(appliedQuery);

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
  let conversionError: string | undefined;
  let draftQuery: ProjectContextQuery | undefined;
  let draftKey: string | undefined;
  if (!validation) {
    try {
      draftQuery = projectContextQueryFromDraft(draft);
      draftKey = projectContextQueryKey(draftQuery);
    } catch (error) {
      conversionError =
        error instanceof Error ? error.message : "The query draft is invalid.";
    }
  }
  const draftError = validation ?? conversionError;
  const dirty = draftKey !== appliedKey;
  const blockingGuidance = runDisabled ? runDisabledReason : draftError;

  return (
    <section
      className={cn(
        panel
          ? "min-h-0"
          : "border-b border-border/70 bg-background/70 px-3 py-3 sm:px-5",
      )}
      data-draft-dirty={dirty}
      data-testid="project-context-query-bar"
    >
      <div className={cn("flex flex-col gap-3", !panel && "mx-auto max-w-6xl")}>
        <div className="flex flex-wrap items-center gap-2">
          <fieldset
            className={cn(
              "flex max-w-full flex-nowrap gap-1 overflow-x-auto rounded-xl border border-border/70 bg-muted/25 p-1",
              panel && "w-full",
            )}
          >
            <legend className="sr-only">Project Context query mode</legend>
            {MODES.map((candidate) => (
              <Button
                aria-pressed={draft.mode === candidate.mode}
                className="shrink-0"
                data-testid={`project-context-mode-${candidate.mode}`}
                key={candidate.mode}
                onClick={() =>
                  onDraftChange((current) =>
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
          <ProjectContextCoordinatePicker
            closeOnSelect={draft.mode === "incident"}
            disabled={
              draft.mode === "all" ||
              (draft.mode === "incident" && draft.coordinates.length === 1)
            }
            documentsState={documentsState}
            meetingsState={meetingsState}
            onSelect={(option) =>
              onDraftChange((current) =>
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
                onDraftChange((current) => ({
                  ...current,
                  coordinates: [],
                }))
              }
              size="sm"
              type="button"
              variant="ghost"
            >
              Clear
            </Button>
          ) : null}
          <div className={cn("min-w-4 flex-1", panel && "hidden")} />
          <Badge variant={dirty ? "warning" : "outline"}>
            {dirty
              ? "Draft · not applied"
              : `Applied · ${modeLabel(draft.mode)}`}
          </Badge>
          <Button
            data-testid="project-context-run-query"
            disabled={
              runDisabled || Boolean(draftError) || !dirty || !draftQuery
            }
            onClick={() => {
              if (draftQuery) onRun(draftQuery);
            }}
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
                      onDraftChange((current) =>
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
            blockingGuidance
              ? "text-amber-700 dark:text-amber-300"
              : "text-muted-foreground",
          )}
          data-testid="project-context-query-guidance"
        >
          {blockingGuidance ??
            (draft.mode === "all"
              ? "All Context is the complete contains-all empty-set query."
              : "Editing this draft does not change the graph until you Run it.")}
        </p>
      </div>
    </section>
  );
}
