import type {
  ProjectView,
  ProjectViewObjectRef,
  ProjectViewObject,
  ProjectViewObjectType,
  ProjectViewWritableObject,
} from "@/shared/api/tauriProjectView";

export type ProjectViewCreateContext = {
  underGoalId?: string;
  underPlanId?: string;
  plannedInStageId?: string;
  handles?: ProjectViewObjectRef;
};

export type ProjectViewIncomingReference = {
  relation:
    | "under goal"
    | "under plan"
    | "planned in stage"
    | "about"
    | "handles";
  source: ProjectViewObject;
};

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
      return object.data.description;
    case "resource":
      return object.data.summary ?? object.data.resourceKind;
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

export function projectViewObjectPaths(view: ProjectView): Map<string, string> {
  const paths = new Map<string, string>();
  const addWork = (
    parentPath: string,
    works: Array<ProjectView["unplannedIssues"][number]["works"][number]>,
  ) => {
    for (const work of works) {
      paths.set(work.id, `${parentPath} / ${work.data.title}`);
    }
  };
  const addPlan = (
    parentPath: string,
    plan: ProjectView["unboundPlans"][number],
  ) => {
    const planPath = `${parentPath} / ${plan.plan.data.title}`;
    paths.set(plan.plan.id, planPath);
    for (const stage of plan.stages) {
      const stagePath = `${planPath} / ${stage.stage.data.title}`;
      paths.set(stage.stage.id, stagePath);
      for (const requirement of stage.requirements) {
        const requirementPath = `${stagePath} / ${requirement.requirement.data.title}`;
        paths.set(requirement.requirement.id, requirementPath);
        addWork(requirementPath, requirement.works);
      }
      for (const issue of stage.issues) {
        const issuePath = `${stagePath} / ${issue.issue.data.title}`;
        paths.set(issue.issue.id, issuePath);
        addWork(issuePath, issue.works);
      }
    }
  };

  paths.set(view.profile.id, view.profile.data.name);
  for (const goal of view.goals) {
    const goalPath = goal.goal.data.title;
    paths.set(goal.goal.id, goalPath);
    goal.plans.forEach((plan) => {
      addPlan(goalPath, plan);
    });
  }
  view.unboundPlans.forEach((plan) => {
    addPlan("Unbound plans", plan);
  });
  for (const requirement of view.unplannedRequirements) {
    const path = `Unplanned requirements / ${requirement.requirement.data.title}`;
    paths.set(requirement.requirement.id, path);
    addWork(path, requirement.works);
  }
  for (const issue of view.unplannedIssues) {
    const path = `Unplanned issues / ${issue.issue.data.title}`;
    paths.set(issue.issue.id, path);
    addWork(path, issue.works);
  }
  view.roles.forEach((role) => {
    paths.set(role.id, `Roles / ${role.data.name}`);
  });
  view.resources.forEach((resource) => {
    paths.set(resource.id, `Resources / ${resource.data.name}`);
  });
  return paths;
}

export function projectViewIncomingReferences(
  view: ProjectView,
  targetId: string,
): ProjectViewIncomingReference[] {
  const references: ProjectViewIncomingReference[] = [];
  for (const source of indexProjectViewObjects(view).values()) {
    if (source.relations.underGoalId === targetId) {
      references.push({ source, relation: "under goal" });
    }
    if (source.relations.underPlanId === targetId) {
      references.push({ source, relation: "under plan" });
    }
    if (source.relations.plannedInStageId === targetId) {
      references.push({ source, relation: "planned in stage" });
    }
    if (source.relations.about?.objectId === targetId) {
      references.push({ source, relation: "about" });
    }
    if (source.relations.handles?.objectId === targetId) {
      references.push({ source, relation: "handles" });
    }
  }
  return references;
}

export function writableProjectViewObject(
  object: ProjectViewObject,
): ProjectViewWritableObject {
  switch (object.objectType) {
    case "project_profile":
      return { objectType: object.objectType, data: object.data };
    case "goal":
      return { objectType: object.objectType, data: object.data };
    case "role":
      return { objectType: object.objectType, data: object.data };
    case "resource":
      return { objectType: object.objectType, data: object.data };
    case "plan":
      return {
        objectType: object.objectType,
        data: object.data,
        underGoalId: object.relations.underGoalId,
      };
    case "stage": {
      const underPlanId = object.relations.underPlanId;
      if (!underPlanId) {
        throw new Error(
          "Project View integrity error: Stage has no parent Plan",
        );
      }
      return {
        objectType: object.objectType,
        data: object.data,
        underPlanId,
      };
    }
    case "requirement":
      return {
        objectType: object.objectType,
        data: object.data,
        plannedInStageId: object.relations.plannedInStageId,
      };
    case "issue":
      return {
        objectType: object.objectType,
        data: object.data,
        plannedInStageId: object.relations.plannedInStageId,
        about: object.relations.about,
      };
    case "work": {
      const handles = object.relations.handles;
      if (!handles) {
        throw new Error(
          "Project View integrity error: Work has no handled object",
        );
      }
      return {
        objectType: object.objectType,
        data: object.data,
        handles,
      };
    }
  }
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
