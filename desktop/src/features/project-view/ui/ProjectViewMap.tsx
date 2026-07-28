import { CircleDot, Layers3 } from "lucide-react";

import { ProjectViewObjectCard } from "@/features/project-view/ui/ProjectViewObjectCard";
import type {
  ProjectView,
  ProjectViewIssue,
  ProjectViewPlan,
  ProjectViewRequirement,
  ProjectViewStage,
} from "@/shared/api/tauriProjectView";

type ProjectViewMapProps = {
  onSelectObject: (objectId: string) => void;
  selectedObjectId?: string;
  view: ProjectView;
};

function ObjectCard({
  object,
  onSelectObject,
  selectedObjectId,
  view,
  size,
}: {
  object: Parameters<typeof ProjectViewObjectCard>[0]["object"];
  onSelectObject: (objectId: string) => void;
  selectedObjectId?: string;
  size?: "default" | "compact";
  view: ProjectView;
}) {
  return (
    <ProjectViewObjectCard
      issueReferenceCount={view.issueReferencesByTarget[object.id]?.length ?? 0}
      object={object}
      onSelect={onSelectObject}
      selected={selectedObjectId === object.id}
      size={size}
    />
  );
}

function WorkItems({
  items,
  ...props
}: Omit<ProjectViewMapProps, "view"> & {
  items: ProjectViewRequirement["works"] | ProjectViewIssue["works"];
  view: ProjectView;
}) {
  if (items.length === 0) return null;
  return (
    <div className="mt-2 border-l border-border/70 pl-2">
      <div className="mb-1.5 text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
        Work
      </div>
      <div className="space-y-2">
        {items.map((work) => (
          <ObjectCard key={work.id} object={work} size="compact" {...props} />
        ))}
      </div>
    </div>
  );
}

function RequirementColumn({
  issues,
  requirements,
  ...props
}: Omit<ProjectViewMapProps, "view"> & {
  issues: ProjectViewStage["issues"];
  requirements: ProjectViewStage["requirements"];
  view: ProjectView;
}) {
  if (requirements.length === 0 && issues.length === 0) {
    return (
      <div className="rounded-lg border border-dashed border-border/70 px-3 py-4 text-center text-xs text-muted-foreground">
        No requirements or issues planned here.
      </div>
    );
  }
  return (
    <div className="grid gap-3 xl:grid-cols-2">
      {requirements.length > 0 ? (
        <div className="space-y-2">
          <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Requirements
          </div>
          {requirements.map((entry) => (
            <div key={entry.requirement.id}>
              <ObjectCard object={entry.requirement} {...props} />
              <WorkItems items={entry.works} {...props} />
            </div>
          ))}
        </div>
      ) : null}
      {issues.length > 0 ? (
        <div className="space-y-2">
          <div className="text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
            Issues
          </div>
          {issues.map((entry) => (
            <div key={entry.issue.id}>
              <ObjectCard object={entry.issue} {...props} />
              <WorkItems items={entry.works} {...props} />
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function StageBranch({
  stage,
  ...props
}: Omit<ProjectViewMapProps, "view"> & {
  stage: ProjectViewStage;
  view: ProjectView;
}) {
  return (
    <div className="relative border-l border-border/70 pl-3">
      <span className="absolute -left-1 top-4 h-2 w-2 rounded-full border border-border bg-background" />
      <ObjectCard object={stage.stage} {...props} />
      <div className="mt-3">
        <RequirementColumn
          issues={stage.issues}
          requirements={stage.requirements}
          {...props}
        />
      </div>
    </div>
  );
}

function PlanBranch({
  plan,
  ...props
}: Omit<ProjectViewMapProps, "view"> & {
  plan: ProjectViewPlan;
  view: ProjectView;
}) {
  return (
    <div className="rounded-2xl border border-border/60 bg-muted/20 p-3">
      <ObjectCard object={plan.plan} {...props} />
      {plan.stages.length > 0 ? (
        <div className="mt-3 space-y-3 pl-2">
          {plan.stages.map((stage) => (
            <StageBranch key={stage.stage.id} stage={stage} {...props} />
          ))}
        </div>
      ) : (
        <div className="mt-3 rounded-lg border border-dashed border-border/70 px-3 py-3 text-xs text-muted-foreground">
          No stages in this plan.
        </div>
      )}
    </div>
  );
}

function LooseRequirement({
  entry,
  ...props
}: Omit<ProjectViewMapProps, "view"> & {
  entry: ProjectViewRequirement;
  view: ProjectView;
}) {
  return (
    <div>
      <ObjectCard object={entry.requirement} {...props} />
      <WorkItems items={entry.works} {...props} />
    </div>
  );
}

function LooseIssue({
  entry,
  ...props
}: Omit<ProjectViewMapProps, "view"> & {
  entry: ProjectViewIssue;
  view: ProjectView;
}) {
  return (
    <div>
      <ObjectCard object={entry.issue} {...props} />
      <WorkItems items={entry.works} {...props} />
    </div>
  );
}

export function ProjectViewMap({
  onSelectObject,
  selectedObjectId,
  view,
}: ProjectViewMapProps) {
  const shared = { onSelectObject, selectedObjectId, view };
  const hasLooseObjects =
    view.unboundPlans.length > 0 ||
    view.unplannedRequirements.length > 0 ||
    view.unplannedIssues.length > 0;

  return (
    <div className="space-y-6" data-testid="project-view-map">
      <section>
        <div className="mb-3 flex items-center gap-2">
          <Layers3 className="h-4 w-4 text-muted-foreground" />
          <h2 className="text-base font-semibold">Project map</h2>
          <span className="text-xs text-muted-foreground">
            Goal → Plan → Stage → Requirement or Issue → Work
          </span>
        </div>
        <div className="space-y-4">
          {view.goals.map(({ goal, plans }) => (
            <article
              className="rounded-2xl border border-border/70 bg-background/70 p-4 shadow-xs"
              key={goal.id}
            >
              <ObjectCard object={goal} {...shared} />
              {plans.length > 0 ? (
                <div className="mt-3 grid gap-3 2xl:grid-cols-2">
                  {plans.map((plan) => (
                    <PlanBranch key={plan.plan.id} plan={plan} {...shared} />
                  ))}
                </div>
              ) : (
                <div className="mt-3 rounded-xl border border-dashed border-border/70 px-3 py-4 text-center text-xs text-muted-foreground">
                  This goal has no bound plan.
                </div>
              )}
            </article>
          ))}
        </div>
      </section>

      {hasLooseObjects ? (
        <section
          className="rounded-2xl border border-dashed border-border bg-muted/10 p-4"
          data-testid="project-view-unplanned"
        >
          <div className="mb-3 flex items-center gap-2">
            <CircleDot className="h-4 w-4 text-muted-foreground" />
            <div>
              <h2 className="text-sm font-semibold">Not yet placed</h2>
              <p className="text-xs text-muted-foreground">
                Valid project objects that do not currently belong to a goal or
                stage.
              </p>
            </div>
          </div>
          <div className="space-y-4">
            {view.unboundPlans.length > 0 ? (
              <div>
                <div className="mb-2 text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                  Unbound plans
                </div>
                <div className="grid gap-3 2xl:grid-cols-2">
                  {view.unboundPlans.map((plan) => (
                    <PlanBranch key={plan.plan.id} plan={plan} {...shared} />
                  ))}
                </div>
              </div>
            ) : null}
            {view.unplannedRequirements.length > 0 ? (
              <div>
                <div className="mb-2 text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                  Unplanned requirements
                </div>
                <div className="grid gap-3 xl:grid-cols-2">
                  {view.unplannedRequirements.map((entry) => (
                    <LooseRequirement
                      entry={entry}
                      key={entry.requirement.id}
                      {...shared}
                    />
                  ))}
                </div>
              </div>
            ) : null}
            {view.unplannedIssues.length > 0 ? (
              <div>
                <div className="mb-2 text-2xs font-semibold uppercase tracking-wider text-muted-foreground">
                  Unplanned issues
                </div>
                <div className="grid gap-3 xl:grid-cols-2">
                  {view.unplannedIssues.map((entry) => (
                    <LooseIssue
                      entry={entry}
                      key={entry.issue.id}
                      {...shared}
                    />
                  ))}
                </div>
              </div>
            ) : null}
          </div>
        </section>
      ) : null}
    </div>
  );
}
