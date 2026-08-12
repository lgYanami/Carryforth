import { buildProjectContextGraph } from "@/features/project-context/graph";
import type {
  ProjectContextQuery,
  ProjectContextQueryResult,
} from "@/shared/api/tauriProjectContext";

function queryLabel(query: ProjectContextQuery) {
  if (query.type === "incident") return "Incident · 1 Coordinate";
  const count = query.coordinates.length;
  if (query.type === "contains_all" && count === 0) return "All Context";
  const mode = query.type === "exact" ? "Exact" : "Contains all";
  return `${mode} · ${count} ${count === 1 ? "Coordinate" : "Coordinates"}`;
}

/** Compact verified substrate summary for the Structure tool pane. */
export function ProjectContextStructureOverview({
  appliedQuery,
  displayedForSemanticResult,
  result,
}: {
  appliedQuery: ProjectContextQuery;
  displayedForSemanticResult: boolean;
  result?: ProjectContextQueryResult;
}) {
  if (!result) return null;
  const graph = buildProjectContextGraph(result);
  const contextDocumentCount = new Set(
    graph.hubs.flatMap((hub) => hub.contextDocumentIds),
  ).size;

  return (
    <section
      className="mb-4 space-y-3 rounded-xl border border-border/70 bg-muted/20 p-3"
      data-testid="project-context-structure-overview"
    >
      <h3 className="text-sm font-semibold">Verified canvas</h3>
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
        <dt className="text-muted-foreground">Applied route query</dt>
        <dd className="min-w-0 text-right font-medium">
          {queryLabel(appliedQuery)}
        </dd>
        <dt className="text-muted-foreground">Displayed canvas</dt>
        <dd className="min-w-0 text-right font-medium">
          {queryLabel(result.query)}
          {displayedForSemanticResult ? " · semantic result" : ""}
        </dd>
      </dl>
      <dl
        className="grid grid-cols-3 gap-2 text-center"
        data-testid="project-context-structure-counts"
      >
        <div className="rounded-lg bg-background/80 px-2 py-2">
          <dt className="text-2xs uppercase tracking-wider text-muted-foreground">
            {graph.isAllContext ? "Edges" : "Matching"}
          </dt>
          <dd className="text-base font-semibold">{graph.hubs.length}</dd>
        </div>
        <div className="rounded-lg bg-background/80 px-2 py-2">
          <dt className="text-2xs uppercase tracking-wider text-muted-foreground">
            Coordinates
          </dt>
          <dd className="text-base font-semibold">
            {graph.coordinates.length}
          </dd>
        </div>
        <div className="rounded-lg bg-background/80 px-2 py-2">
          <dt className="text-2xs uppercase tracking-wider text-muted-foreground">
            Context docs
          </dt>
          <dd className="text-base font-semibold">{contextDocumentCount}</dd>
        </div>
      </dl>
      <p className="text-xs text-muted-foreground">
        {graph.isAllContext
          ? `${graph.islands.length} verified ${graph.islands.length === 1 ? "Context Island" : "Context Islands"}. Use the canvas Island controls to focus one without changing the query.`
          : "This focused result shows matching Edges and shared query anchors; it is not a project-level Island count."}
      </p>
    </section>
  );
}
