import { CircleAlert, LocateFixed, Network, Search, X } from "lucide-react";
import * as React from "react";

import {
  removeSemanticQueryDraftCoordinate,
  semanticGraphCountLabel,
  semanticQueryDraftMatchesSubmission,
  tryAddSemanticQueryDraftCoordinate,
  updateSemanticQueryDraftProblem,
  validateSemanticQueryDraft,
  type SemanticQueryDraft,
  type SemanticQueryCoordinateRole,
} from "@/features/project-context/semanticQueryModel";
import type {
  SemanticAttempt,
  SemanticSession,
} from "@/features/project-context/semanticSession";
import type { ProjectContextCoordinateOption } from "@/features/project-context/queryModel";
import {
  ProjectContextCoordinatePicker,
  type ProjectContextPickerSourceState,
} from "@/features/project-context/ui/ProjectContextCoordinatePicker";
import {
  projectContextCoordinateKey,
  type ProjectContextCoordinate,
} from "@/shared/api/tauriProjectContext";
import {
  SEMANTIC_PROJECT_CONTEXT_MAX_CONTEXT_COORDINATES,
  SEMANTIC_PROJECT_CONTEXT_MAX_INITIAL_COORDINATES,
  SEMANTIC_PROJECT_CONTEXT_MAX_PROBLEM_BYTES,
  type SemanticProjectContextQueryResult,
} from "@/shared/api/tauriProjectContextSemantic";
import { cn } from "@/shared/lib/cn";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

const ACTIVE_PROBLEM_SUMMARY_CODE_POINTS = 160;

function semanticProblemSummary(problem: string): string {
  const normalized = problem.replace(/\s+/g, " ").trim();
  const codePoints = [...normalized];
  if (codePoints.length <= ACTIVE_PROBLEM_SUMMARY_CODE_POINTS) {
    return normalized;
  }
  return `${codePoints
    .slice(0, ACTIVE_PROBLEM_SUMMARY_CODE_POINTS - 1)
    .join("")}…`;
}

function semanticAttemptLabel(attempt: SemanticAttempt): string | undefined {
  if (attempt.status === "running") return "Finding semantic paths…";
  if (attempt.status === "pairing") {
    return "Pairing verified paths with All Context…";
  }
  return undefined;
}

function CoordinateChips({
  coordinateOptions,
  coordinates,
  coordinateRole,
  onRemove,
}: {
  coordinateOptions: ProjectContextCoordinateOption[];
  coordinates: ProjectContextCoordinate[];
  coordinateRole: SemanticQueryCoordinateRole;
  onRemove: (role: SemanticQueryCoordinateRole, key: string) => void;
}) {
  const optionByKey = React.useMemo(
    () =>
      new Map(
        coordinateOptions.map((option) => [option.coordinateKey, option]),
      ),
    [coordinateOptions],
  );
  if (coordinates.length === 0) return null;
  return (
    <ul
      aria-label={
        coordinateRole === "initial"
          ? "Initial Coordinates"
          : "Context Coordinates"
      }
      className="mt-2 flex list-none flex-wrap gap-1.5 p-0"
      data-testid={`project-context-semantic-${coordinateRole}-chips`}
    >
      {coordinates.map((coordinate) => {
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
              aria-label={`Remove ${option?.title ?? key} from ${coordinateRole} Coordinates`}
              className="rounded-full p-0.5 text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              onClick={() => onRemove(coordinateRole, key)}
              type="button"
            >
              <X className="h-3 w-3" />
            </button>
          </li>
        );
      })}
    </ul>
  );
}

/** Problem-first semantic path controls with explicit optional graph inputs. */
export function ProjectContextSemanticQueryBar<TOverlay>({
  active,
  attempt,
  available,
  canFit,
  coordinateOptions,
  documentsState,
  draft,
  freshness,
  meetingsState,
  onCancel,
  onDraftChange,
  onFit,
  onRun,
  overlayVisible,
  panel = false,
  projectViewState,
  topologyAdvanced,
}: {
  active: SemanticSession<TOverlay> | null;
  attempt: SemanticAttempt;
  available: boolean;
  canFit: boolean;
  coordinateOptions: ProjectContextCoordinateOption[];
  documentsState: ProjectContextPickerSourceState;
  draft: SemanticQueryDraft;
  freshness: "snapshot" | "stale";
  meetingsState: ProjectContextPickerSourceState;
  onCancel: () => void;
  onDraftChange: (draft: SemanticQueryDraft) => void;
  onFit: () => void;
  onRun: () => void;
  overlayVisible: boolean;
  panel?: boolean;
  projectViewState: ProjectContextPickerSourceState;
  topologyAdvanced: boolean;
}) {
  const [problemTouched, setProblemTouched] = React.useState(false);
  const validation = validateSemanticQueryDraft(draft);
  const showProblemValidation = !validation.valid && problemTouched;
  const inFlight = attempt.status === "running" || attempt.status === "pairing";
  const activeResult: SemanticProjectContextQueryResult | undefined =
    active?.verifiedDisplayResult;
  const activeProblemSummary = active
    ? semanticProblemSummary(active.submittedDraft.problem)
    : undefined;
  const dirty = active
    ? !semanticQueryDraftMatchesSubmission(draft, active.submittedDraft)
    : draft.problem.length > 0 ||
      draft.initialCoordinates.length > 0 ||
      draft.contextCoordinates.length > 0;
  const attemptLabel = semanticAttemptLabel(attempt);
  const omittedInputCount = activeResult
    ? activeResult.coverage.omittedInitialCoordinates +
      activeResult.coverage.omittedContextCoordinates
    : 0;
  const responseBudgetOmissions = activeResult
    ? activeResult.coverage.omittedForResponseBudget.automaticRoots +
      activeResult.coverage.omittedForResponseBudget.paths +
      activeResult.coverage.omittedForResponseBudget.summaries
    : 0;
  const partialCoverage = activeResult
    ? activeResult.coverage.currentIndexedGraphSources <
        activeResult.coverage.authorizedGraphSources ||
      activeResult.coverage.indexCoveragePartial > 0 ||
      omittedInputCount > 0 ||
      responseBudgetOmissions > 0
    : false;
  const budgetExhausted = activeResult
    ? activeResult.completionReason !== "frontier_exhausted" ||
      activeResult.exhaustedDimensions.length > 0
    : false;

  const selectedInitial = React.useMemo(
    () =>
      new Set(
        draft.initialCoordinates.map((coordinate) =>
          projectContextCoordinateKey(coordinate),
        ),
      ),
    [draft.initialCoordinates],
  );
  const selectedContext = React.useMemo(
    () =>
      new Set(
        draft.contextCoordinates.map((coordinate) =>
          projectContextCoordinateKey(coordinate),
        ),
      ),
    [draft.contextCoordinates],
  );

  function addCoordinate(
    role: SemanticQueryCoordinateRole,
    option: ProjectContextCoordinateOption,
  ) {
    const transition = tryAddSemanticQueryDraftCoordinate(
      draft,
      role,
      option.coordinate,
    );
    if (transition.status === "changed") onDraftChange(transition.draft);
  }

  function removeCoordinate(role: SemanticQueryCoordinateRole, key: string) {
    onDraftChange(removeSemanticQueryDraftCoordinate(draft, role, key));
  }

  const canRun = available && validation.valid && !inFlight;
  return (
    <section
      className={cn(
        panel
          ? "min-h-0"
          : "border-b border-border/70 bg-background/85 px-3 py-3 sm:px-5",
      )}
      data-active={active ? "true" : "false"}
      data-draft-dirty={dirty}
      data-testid="project-context-semantic-query-bar"
    >
      <div className={cn("flex flex-col gap-3", !panel && "mx-auto max-w-6xl")}>
        <div className="flex items-center gap-2">
          <Search className="h-4 w-4 shrink-0 text-primary" />
          <div className="min-w-0 flex-1">
            <h2 className="text-sm font-semibold">Semantic paths</h2>
            <p className="text-xs text-muted-foreground">
              Ask a project question, then optionally choose where traversal
              starts and which Coordinates shape relevance.
            </p>
          </div>
          {active ? (
            <Badge
              data-testid="project-context-semantic-active-badge"
              variant={
                topologyAdvanced || freshness === "stale"
                  ? "warning"
                  : "success"
              }
            >
              {topologyAdvanced
                ? "Context changed"
                : freshness === "stale"
                  ? "Snapshot · stale"
                  : "Snapshot"}
            </Badge>
          ) : null}
        </div>

        <div
          className={cn(
            "flex gap-2",
            panel ? "flex-col items-stretch" : "items-end",
          )}
        >
          <label className="min-w-0 flex-1">
            <span className="sr-only">Project problem</span>
            <textarea
              aria-describedby="project-context-semantic-guidance project-context-semantic-problem-bytes"
              aria-invalid={showProblemValidation}
              className="min-h-20 w-full resize-y rounded-xl border border-border bg-background px-3 py-2 text-sm leading-6 outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring"
              data-testid="project-context-semantic-problem"
              onBlur={() => setProblemTouched(true)}
              onChange={(event) =>
                onDraftChange(
                  updateSemanticQueryDraftProblem(draft, event.target.value),
                )
              }
              onKeyDown={(event) => {
                if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                  event.preventDefault();
                  setProblemTouched(true);
                  if (canRun) onRun();
                }
              }}
              placeholder="For example: Why did this release issue keep recurring?"
              value={draft.problem}
            />
          </label>
          <Button
            className={panel ? "w-full" : undefined}
            data-testid="project-context-semantic-run"
            disabled={!canRun}
            onClick={onRun}
            type="button"
          >
            <Network />
            {active ? "Re-run" : "Find paths"}
          </Button>
        </div>
        <div
          className="-mt-2 text-right text-2xs text-muted-foreground"
          id="project-context-semantic-problem-bytes"
        >
          {validation.problemBytes.toLocaleString()} /{" "}
          {SEMANTIC_PROJECT_CONTEXT_MAX_PROBLEM_BYTES.toLocaleString()} UTF-8
          bytes
        </div>

        <details className="rounded-xl border border-border/70 bg-muted/15 px-3 py-2">
          <summary className="cursor-pointer text-xs font-semibold">
            Optional graph inputs
          </summary>
          <div className={cn("mt-3 grid gap-4", !panel && "lg:grid-cols-2")}>
            <fieldset>
              <legend className="sr-only">Start from</legend>
              <div className="flex flex-wrap items-center gap-2">
                <div className="min-w-0 flex-1">
                  <h3 className="text-xs font-semibold">Start from</h3>
                  <p className="text-xs text-muted-foreground">
                    Explicit traversal roots. Leave empty to discover roots
                    semantically.
                  </p>
                </div>
                <ProjectContextCoordinatePicker
                  buttonLabel="Add starting point"
                  closeOnSelect={false}
                  disabled={
                    draft.initialCoordinates.length >=
                    SEMANTIC_PROJECT_CONTEXT_MAX_INITIAL_COORDINATES
                  }
                  documentsState={documentsState}
                  meetingsState={meetingsState}
                  onSelect={(option) => addCoordinate("initial", option)}
                  options={coordinateOptions}
                  pickerTestId="project-context-semantic-initial-picker"
                  projectViewState={projectViewState}
                  searchLabel="Search initial Project Coordinates"
                  searchTestId="project-context-semantic-initial-search"
                  selectedKeys={selectedInitial}
                />
              </div>
              <CoordinateChips
                coordinateOptions={coordinateOptions}
                coordinates={draft.initialCoordinates}
                coordinateRole="initial"
                onRemove={removeCoordinate}
              />
              {draft.initialCoordinates.length >=
              SEMANTIC_PROJECT_CONTEXT_MAX_INITIAL_COORDINATES ? (
                <p className="mt-1 text-2xs text-muted-foreground">
                  Maximum {SEMANTIC_PROJECT_CONTEXT_MAX_INITIAL_COORDINATES}{" "}
                  starting Coordinates selected.
                </p>
              ) : null}
            </fieldset>
            <fieldset>
              <legend className="sr-only">Query context</legend>
              <div className="flex flex-wrap items-center gap-2">
                <div className="min-w-0 flex-1">
                  <h3 className="text-xs font-semibold">Query context</h3>
                  <p className="text-xs text-muted-foreground">
                    Shapes relevance ranking; it is not a filter, permission, or
                    required starting point.
                  </p>
                </div>
                <ProjectContextCoordinatePicker
                  buttonLabel="Add query context"
                  closeOnSelect={false}
                  disabled={
                    draft.contextCoordinates.length >=
                    SEMANTIC_PROJECT_CONTEXT_MAX_CONTEXT_COORDINATES
                  }
                  documentsState={documentsState}
                  meetingsState={meetingsState}
                  onSelect={(option) => addCoordinate("context", option)}
                  options={coordinateOptions}
                  pickerTestId="project-context-semantic-context-picker"
                  projectViewState={projectViewState}
                  searchLabel="Search context Project Coordinates"
                  searchTestId="project-context-semantic-context-search"
                  selectedKeys={selectedContext}
                />
              </div>
              <CoordinateChips
                coordinateOptions={coordinateOptions}
                coordinates={draft.contextCoordinates}
                coordinateRole="context"
                onRemove={removeCoordinate}
              />
              {draft.contextCoordinates.length >=
              SEMANTIC_PROJECT_CONTEXT_MAX_CONTEXT_COORDINATES ? (
                <p className="mt-1 text-2xs text-muted-foreground">
                  Maximum {SEMANTIC_PROJECT_CONTEXT_MAX_CONTEXT_COORDINATES}{" "}
                  query-context Coordinates selected.
                </p>
              ) : null}
            </fieldset>
          </div>
        </details>

        {active && activeResult ? (
          <div
            className="flex flex-wrap items-center gap-2 rounded-xl border border-border/70 bg-card/55 px-3 py-2 text-xs"
            data-testid="project-context-semantic-result-status"
          >
            <span className="min-w-0 max-w-full flex-1 truncate font-medium">
              Semantic snapshot · “{activeProblemSummary}”
            </span>
            <Badge variant="outline">
              {activeResult.coverage.pathsReturned === 0
                ? "No paths"
                : semanticGraphCountLabel(
                    activeResult.coverage.pathsReturned,
                    "path",
                  )}
            </Badge>
            <Badge variant="outline">
              {semanticGraphCountLabel(
                activeResult.coverage.rootsReturned,
                "root",
              )}
            </Badge>
            <Badge variant="outline">
              Revision {activeResult.projectContextRevision}
            </Badge>
            {dirty ? (
              <Badge variant="warning">Draft · not applied</Badge>
            ) : null}
            {partialCoverage ? (
              <Badge variant="warning">Partial coverage</Badge>
            ) : (
              <Badge variant="success">No coverage omissions</Badge>
            )}
            {budgetExhausted ? (
              <Badge variant="warning">Budget reached</Badge>
            ) : null}
            {omittedInputCount > 0 ? (
              <Badge variant="warning">
                {omittedInputCount} inputs omitted
              </Badge>
            ) : null}
            <time
              className="text-muted-foreground"
              dateTime={activeResult.snapshotObservedAt}
              title={activeResult.snapshotObservedAt}
            >
              Observed{" "}
              {new Date(activeResult.snapshotObservedAt).toLocaleString()}
            </time>
          </div>
        ) : null}

        <div className="flex flex-wrap items-center gap-2">
          <p
            className={cn(
              "min-w-0 flex-1 text-xs",
              showProblemValidation || attempt.status === "failed"
                ? "text-amber-700 dark:text-amber-300"
                : "text-muted-foreground",
            )}
            data-testid="project-context-semantic-guidance"
            id="project-context-semantic-guidance"
          >
            {!available
              ? "Semantic query is not available for this Community."
              : showProblemValidation
                ? validation.message
                : attempt.status === "failed"
                  ? attempt.error.message
                  : (attemptLabel ??
                    (topologyAdvanced
                      ? "Context changed after this semantic snapshot. Re-run to find paths in the current graph, or clear the result."
                      : activeResult
                        ? `${semanticGraphCountLabel(activeResult.coverage.pathsReturned, "path")} ${overlayVisible ? "highlighted" : "retained but hidden because the graph changed"} from Context revision ${activeResult.projectContextRevision}.${dirty ? " Draft changes are not applied." : ""}`
                        : "Cmd/Ctrl+Enter also runs the query. Results are routing candidates; open canonical sources before relying on them."))}
          </p>
          {active && overlayVisible && canFit ? (
            <Button
              data-testid="project-context-semantic-fit"
              onClick={onFit}
              size="sm"
              type="button"
              variant="outline"
            >
              <LocateFixed />
              Fit paths
            </Button>
          ) : null}
          {active || inFlight ? (
            <Button
              data-testid="project-context-semantic-cancel"
              onClick={onCancel}
              size="sm"
              type="button"
              variant="ghost"
            >
              <X />
              {inFlight && !active ? "Cancel search" : "Clear semantic result"}
            </Button>
          ) : attempt.status === "failed" ? (
            <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
              <CircleAlert className="h-3.5 w-3.5" />
              Submit again to retry; Desktop never retries automatically.
            </span>
          ) : null}
        </div>
      </div>
    </section>
  );
}
