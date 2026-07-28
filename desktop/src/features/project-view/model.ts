import type {
  ProjectView,
  ProjectViewObject,
  ProjectViewObjectType,
} from "@/shared/api/tauriProjectView";

const OBJECT_TYPE_LABELS: Record<ProjectViewObjectType, string> = {
  project_profile: "Project",
  goal: "Goal",
  role: "Role",
  plan: "Plan",
  stage: "Stage",
  requirement: "Requirement",
  issue: "Issue",
  work: "Work",
  resource: "Resource",
};

export function projectViewObjectTypeLabel(type: ProjectViewObjectType) {
  return OBJECT_TYPE_LABELS[type];
}

export function projectViewObjectTitle(object: ProjectViewObject): string {
  switch (object.objectType) {
    case "project_profile":
    case "role":
    case "resource":
      return object.data.name;
    case "goal":
    case "plan":
    case "stage":
    case "requirement":
    case "issue":
    case "work":
      return object.data.title;
  }
}

export function projectViewObjectDescription(
  object: ProjectViewObject,
): string {
  switch (object.objectType) {
    case "project_profile":
      return object.data.purpose;
    case "goal":
      return object.data.desiredOutcome;
    case "role":
      return object.data.purpose;
    case "plan":
    case "stage":
    case "requirement":
    case "issue":
    case "work":
    case "resource":
      return object.data.description;
  }
}

export function projectViewObjectStatus(
  object: ProjectViewObject,
): string | undefined {
  switch (object.objectType) {
    case "plan":
    case "stage":
    case "requirement":
    case "issue":
    case "work":
      return object.data.status;
    case "role":
      return object.data.active ? "active" : "inactive";
    default:
      return undefined;
  }
}

export function projectViewObjectPriority(
  object: ProjectViewObject,
): string | undefined {
  switch (object.objectType) {
    case "requirement":
    case "issue":
    case "work":
      return object.data.priority;
    default:
      return undefined;
  }
}

export function formatProjectViewTerm(value: string): string {
  return value
    .split("_")
    .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

export function indexProjectViewObjects(
  view: ProjectView,
): Map<string, ProjectViewObject> {
  const objects = new Map<string, ProjectViewObject>();
  const add = (object: ProjectViewObject) => objects.set(object.id, object);
  const addPlan = (plan: ProjectView["unboundPlans"][number]) => {
    add(plan.plan);
    for (const stage of plan.stages) {
      add(stage.stage);
      for (const requirement of stage.requirements) {
        add(requirement.requirement);
        requirement.works.forEach(add);
      }
      for (const issue of stage.issues) {
        add(issue.issue);
        issue.works.forEach(add);
      }
    }
  };

  add(view.profile);
  for (const goal of view.goals) {
    add(goal.goal);
    goal.plans.forEach(addPlan);
  }
  view.unboundPlans.forEach(addPlan);
  for (const requirement of view.unplannedRequirements) {
    add(requirement.requirement);
    requirement.works.forEach(add);
  }
  for (const issue of view.unplannedIssues) {
    add(issue.issue);
    issue.works.forEach(add);
  }
  view.roles.forEach(add);
  view.resources.forEach(add);
  return objects;
}

export function countProjectViewFocus(view: ProjectView) {
  const objects = indexProjectViewObjects(view);
  let activePlans = 0;
  let activeStages = 0;
  let openIssues = 0;
  let inProgressWork = 0;

  for (const object of objects.values()) {
    if (object.objectType === "plan" && object.data.status === "active") {
      activePlans += 1;
    } else if (
      object.objectType === "stage" &&
      object.data.status === "active"
    ) {
      activeStages += 1;
    } else if (
      object.objectType === "issue" &&
      (object.data.status === "open" || object.data.status === "in_progress")
    ) {
      openIssues += 1;
    } else if (
      object.objectType === "work" &&
      object.data.status === "in_progress"
    ) {
      inProgressWork += 1;
    }
  }

  return { activePlans, activeStages, openIssues, inProgressWork };
}
