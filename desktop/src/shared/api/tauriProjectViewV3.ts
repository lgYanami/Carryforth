import { ProjectViewIntegrityError } from "@/shared/api/tauriProjectViewIntegrity";
import { requireCanonicalProjectViewContextReferences } from "@/shared/api/tauriProjectViewContext";
import type {
  ProjectView,
  ProjectViewContextReference,
  ProjectViewIssue,
  ProjectViewObject,
  ProjectViewObjectOf,
  ProjectViewObjectRef,
  ProjectViewObjectType,
  ProjectViewPlan,
  ProjectViewRelations,
  ProjectViewRequirement,
  ProjectViewStage,
  RawProjectViewContextReference,
  RawProjectViewObjectRef,
  RawProjectViewObjectV3,
  RawProjectViewObjectV3Of,
  RawProjectViewRelations,
} from "@/shared/api/tauriProjectView";

function normalizeRef(raw: RawProjectViewObjectRef): ProjectViewObjectRef {
  return {
    objectType: raw.object_type,
    objectId: raw.object_id,
  };
}

function normalizeRelations(
  raw: RawProjectViewRelations,
): ProjectViewRelations {
  return {
    underGoalId: raw.under_goal_id,
    underPlanId: raw.under_plan_id,
    plannedInStageId: raw.planned_in_stage_id,
    about: raw.about ? normalizeRef(raw.about) : undefined,
    handles: raw.handles ? normalizeRef(raw.handles) : undefined,
  };
}

function normalizeContextReference(
  raw: RawProjectViewContextReference,
): ProjectViewContextReference {
  return raw.type === "resource"
    ? { referenceType: "resource", resourceId: raw.resource_id }
    : {
        referenceType: "document",
        documentId: raw.document_id,
        mode: raw.mode,
        documentRevision: raw.document_revision,
      };
}

function commonV3ObjectFields<T extends ProjectViewObjectType>(
  raw: RawProjectViewObjectV3Of<T>,
) {
  if (raw.data.object_type !== raw.object_type) {
    throw new ProjectViewIntegrityError(
      "v3 object type does not match its data",
    );
  }
  return {
    id: raw.id,
    objectRevision: raw.object_revision,
    projectRevision: raw.project_revision,
    createdAt: raw.created_at,
    updatedAt: raw.updated_at,
    createdBy: raw.created_by,
    updatedBy: raw.updated_by,
    relations: normalizeRelations(raw.relations),
    contextReferences: requireCanonicalProjectViewContextReferences(
      raw.context_references.map(normalizeContextReference),
    ),
  };
}

export function normalizeProjectViewObjectV3(
  raw: RawProjectViewObjectV3,
): ProjectViewObject {
  switch (raw.object_type) {
    case "project_profile":
      return {
        ...commonV3ObjectFields(raw),
        objectType: raw.object_type,
        data: raw.data.data,
      };
    case "goal":
      return {
        ...commonV3ObjectFields(raw),
        objectType: raw.object_type,
        data: {
          title: raw.data.data.title,
          desiredOutcome: raw.data.data.desired_outcome,
          directions: raw.data.data.directions,
        },
      };
    case "role":
    case "plan":
    case "stage":
    case "requirement":
    case "issue":
    case "work":
      return {
        ...commonV3ObjectFields(raw),
        objectType: raw.object_type,
        data: raw.data.data,
      } as ProjectViewObject;
    case "resource":
      return {
        ...commonV3ObjectFields(raw),
        objectType: raw.object_type,
        data: {
          name: raw.data.data.name,
          resourceKind: raw.data.data.resource_kind,
          summary: raw.data.data.summary,
          guideDocumentId: raw.data.data.guide_document_id,
        },
      };
  }
}

function projectViewObjectOrder(
  left: ProjectViewObject,
  right: ProjectViewObject,
) {
  return (
    left.createdAt.localeCompare(right.createdAt) ||
    left.id.localeCompare(right.id)
  );
}

export function assembleProjectViewV3(
  rawObjects: RawProjectViewObjectV3[],
): ProjectView {
  const objects = rawObjects.map(normalizeProjectViewObjectV3);
  const byId = new Map<string, ProjectViewObject>();
  for (const object of objects) {
    if (byId.has(object.id)) {
      throw new ProjectViewIntegrityError(
        `v3 object ${object.id} appears more than once`,
      );
    }
    byId.set(object.id, object);
  }
  const profiles = objects.filter(
    (object): object is ProjectViewObjectOf<"project_profile"> =>
      object.objectType === "project_profile",
  );
  if (profiles.length !== 1) {
    throw new ProjectViewIntegrityError(
      "a v3 snapshot must contain exactly one Project Profile",
    );
  }

  const goals = new Map<
    string,
    { goal: ProjectViewObjectOf<"goal">; plans: ProjectViewPlan[] }
  >();
  const plans = new Map<string, ProjectViewPlan>();
  const stages = new Map<string, ProjectViewStage>();
  const requirements = new Map<string, ProjectViewRequirement>();
  const issues = new Map<string, ProjectViewIssue>();
  const roles: Array<ProjectViewObjectOf<"role">> = [];
  const resources: Array<ProjectViewObjectOf<"resource">> = [];
  for (const object of objects) {
    switch (object.objectType) {
      case "project_profile":
      case "work":
        break;
      case "goal":
        goals.set(object.id, { goal: object, plans: [] });
        break;
      case "plan":
        plans.set(object.id, { plan: object, stages: [] });
        break;
      case "stage":
        stages.set(object.id, { stage: object, requirements: [], issues: [] });
        break;
      case "requirement":
        requirements.set(object.id, { requirement: object, works: [] });
        break;
      case "issue":
        issues.set(object.id, { issue: object, works: [] });
        break;
      case "role":
        roles.push(object);
        break;
      case "resource":
        resources.push(object);
        break;
    }
  }

  for (const object of objects) {
    if (object.objectType !== "work") continue;
    const handles = object.relations.handles;
    const target =
      handles?.objectType === "requirement"
        ? requirements.get(handles.objectId)
        : handles?.objectType === "issue"
          ? issues.get(handles.objectId)
          : undefined;
    if (!target) {
      throw new ProjectViewIntegrityError(
        `v3 Work ${object.id} has no active Handles target`,
      );
    }
    target.works.push(object);
  }

  const unplannedRequirements: ProjectViewRequirement[] = [];
  for (const requirement of requirements.values()) {
    const stageId = requirement.requirement.relations.plannedInStageId;
    if (stageId) {
      const stage = stages.get(stageId);
      if (!stage) {
        throw new ProjectViewIntegrityError(
          `v3 Requirement ${requirement.requirement.id} references a missing Stage`,
        );
      }
      stage.requirements.push(requirement);
    } else {
      unplannedRequirements.push(requirement);
    }
  }
  const unplannedIssues: ProjectViewIssue[] = [];
  for (const issue of issues.values()) {
    const stageId = issue.issue.relations.plannedInStageId;
    if (stageId) {
      const stage = stages.get(stageId);
      if (!stage) {
        throw new ProjectViewIntegrityError(
          `v3 Issue ${issue.issue.id} references a missing Stage`,
        );
      }
      stage.issues.push(issue);
    } else {
      unplannedIssues.push(issue);
    }
  }
  for (const stage of stages.values()) {
    const planId = stage.stage.relations.underPlanId;
    const plan = planId ? plans.get(planId) : undefined;
    if (!plan) {
      throw new ProjectViewIntegrityError(
        `v3 Stage ${stage.stage.id} references a missing Plan`,
      );
    }
    plan.stages.push(stage);
  }
  const unboundPlans: ProjectViewPlan[] = [];
  for (const plan of plans.values()) {
    const goalId = plan.plan.relations.underGoalId;
    if (goalId) {
      const goal = goals.get(goalId);
      if (!goal) {
        throw new ProjectViewIntegrityError(
          `v3 Plan ${plan.plan.id} references a missing Goal`,
        );
      }
      goal.plans.push(plan);
    } else {
      unboundPlans.push(plan);
    }
  }

  const objectOrder = <T extends ProjectViewObject>(left: T, right: T) =>
    projectViewObjectOrder(left, right);
  for (const requirement of requirements.values()) {
    requirement.works.sort(objectOrder);
  }
  for (const issue of issues.values()) issue.works.sort(objectOrder);
  for (const stage of stages.values()) {
    stage.requirements.sort((left, right) =>
      objectOrder(left.requirement, right.requirement),
    );
    stage.issues.sort((left, right) => objectOrder(left.issue, right.issue));
  }
  for (const plan of plans.values()) {
    plan.stages.sort((left, right) => objectOrder(left.stage, right.stage));
  }
  for (const goal of goals.values()) {
    goal.plans.sort((left, right) => objectOrder(left.plan, right.plan));
  }
  const issueReferencesByTarget: Record<string, ProjectViewObjectRef[]> = {};
  for (const issue of issues.values()) {
    const about = issue.issue.relations.about;
    if (!about) continue;
    const references = issueReferencesByTarget[about.objectId] ?? [];
    references.push({
      objectType: "issue",
      objectId: issue.issue.id,
    });
    issueReferencesByTarget[about.objectId] = references;
  }
  for (const references of Object.values(issueReferencesByTarget)) {
    references.sort((left, right) =>
      left.objectId.localeCompare(right.objectId),
    );
  }
  return {
    profile: profiles[0],
    goals: [...goals.values()].sort((left, right) =>
      objectOrder(left.goal, right.goal),
    ),
    unboundPlans: unboundPlans.sort((left, right) =>
      objectOrder(left.plan, right.plan),
    ),
    unplannedRequirements: unplannedRequirements.sort((left, right) =>
      objectOrder(left.requirement, right.requirement),
    ),
    unplannedIssues: unplannedIssues.sort((left, right) =>
      objectOrder(left.issue, right.issue),
    ),
    roles: roles.sort(objectOrder),
    resources: resources.sort(objectOrder),
    issueReferencesByTarget,
  };
}
