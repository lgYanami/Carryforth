import { invokeTauri } from "@/shared/api/tauri";

export type ProjectViewObjectType =
  | "project_profile"
  | "goal"
  | "role"
  | "plan"
  | "stage"
  | "requirement"
  | "issue"
  | "work"
  | "resource";

export type ProjectViewPriority = "low" | "normal" | "high" | "urgent";
export type ProjectViewPlanStatus =
  | "draft"
  | "active"
  | "paused"
  | "completed"
  | "cancelled";
export type ProjectViewStageStatus =
  | "planned"
  | "active"
  | "paused"
  | "completed"
  | "cancelled";
export type ProjectViewRequirementStatus =
  | "proposed"
  | "ready"
  | "in_progress"
  | "satisfied"
  | "withdrawn";
export type ProjectViewIssueStatus =
  | "open"
  | "in_progress"
  | "resolved"
  | "closed";
export type ProjectViewWorkStatus =
  | "pending"
  | "in_progress"
  | "paused"
  | "submitted"
  | "completed"
  | "cancelled";
export type ProjectViewResourceType =
  | "repository"
  | "document"
  | "design"
  | "service"
  | "environment"
  | "artifact"
  | "url";
export type ProjectViewLocatorType =
  | "url"
  | "nostr_address"
  | "nostr_event"
  | "buzz_deep_link";

type RawProjectProfileData = {
  name: string;
  positioning: string;
  purpose: string;
  problem: string;
  scope: string;
};

type RawGoalData = {
  title: string;
  desired_outcome: string;
  directions: string[];
};

type RawRoleData = {
  name: string;
  purpose: string;
  responsibilities: string[];
  boundaries: string[];
  active: boolean;
};

type RawPlanData = {
  title: string;
  description: string;
  status: ProjectViewPlanStatus;
};

type RawStageData = {
  title: string;
  description: string;
  status: ProjectViewStageStatus;
};

type RawRequirementData = {
  title: string;
  description: string;
  status: ProjectViewRequirementStatus;
  priority: ProjectViewPriority;
};

type RawIssueData = {
  title: string;
  description: string;
  status: ProjectViewIssueStatus;
  priority: ProjectViewPriority;
};

type RawWorkData = {
  title: string;
  description: string;
  status: ProjectViewWorkStatus;
  priority: ProjectViewPriority;
};

type RawResourceData = {
  name: string;
  resource_type: ProjectViewResourceType;
  locator: {
    locator_type: ProjectViewLocatorType;
    value: string;
  };
  description: string;
};

type RawDataByType = {
  project_profile: RawProjectProfileData;
  goal: RawGoalData;
  role: RawRoleData;
  plan: RawPlanData;
  stage: RawStageData;
  requirement: RawRequirementData;
  issue: RawIssueData;
  work: RawWorkData;
  resource: RawResourceData;
};

export type RawProjectViewObjectRef = {
  object_type: ProjectViewObjectType;
  object_id: string;
};

export type RawProjectViewRelations = {
  under_goal_id?: string;
  under_plan_id?: string;
  planned_in_stage_id?: string;
  about?: RawProjectViewObjectRef;
  handles?: RawProjectViewObjectRef;
};

type RawProjectViewObjectOf<T extends ProjectViewObjectType> = {
  id: string;
  object_type: T;
  object_revision: number;
  project_revision: number;
  created_at: string;
  updated_at: string;
  created_by: string;
  updated_by: string;
  data: {
    object_type: T;
    data: RawDataByType[T];
  };
  relations: RawProjectViewRelations;
};

export type RawProjectViewObject = {
  [T in ProjectViewObjectType]: RawProjectViewObjectOf<T>;
}[ProjectViewObjectType];

export type RawProjectView = {
  profile: RawProjectViewObjectOf<"project_profile">;
  goals: Array<{
    goal: RawProjectViewObjectOf<"goal">;
    plans: RawPlanView[];
  }>;
  unbound_plans: RawPlanView[];
  unplanned_requirements: RawRequirementView[];
  unplanned_issues: RawIssueView[];
  roles: Array<RawProjectViewObjectOf<"role">>;
  resources: Array<RawProjectViewObjectOf<"resource">>;
  issue_references_by_target: Record<string, RawProjectViewObjectRef[]>;
};

type RawPlanView = {
  plan: RawProjectViewObjectOf<"plan">;
  stages: RawStageView[];
};

type RawStageView = {
  stage: RawProjectViewObjectOf<"stage">;
  requirements: RawRequirementView[];
  issues: RawIssueView[];
};

type RawRequirementView = {
  requirement: RawProjectViewObjectOf<"requirement">;
  works: Array<RawProjectViewObjectOf<"work">>;
};

type RawIssueView = {
  issue: RawProjectViewObjectOf<"issue">;
  works: Array<RawProjectViewObjectOf<"work">>;
};

export type RawProjectViewLoadResult =
  | { status: "unsupported" }
  | { status: "forbidden" }
  | { status: "uninitialized"; relay_pubkey: string }
  | {
      status: "ready";
      relay_pubkey: string;
      project_revision: number;
      projection_generation: number;
      active_object_count: number;
      updated_at: string;
      view: RawProjectView;
    };

export type ProjectProfileData = RawProjectProfileData;
export type ProjectGoalData = {
  title: string;
  desiredOutcome: string;
  directions: string[];
};
export type ProjectRoleData = RawRoleData;
export type ProjectPlanData = RawPlanData;
export type ProjectStageData = RawStageData;
export type ProjectRequirementData = RawRequirementData;
export type ProjectIssueData = RawIssueData;
export type ProjectWorkData = RawWorkData;
export type ProjectResourceData = {
  name: string;
  resourceType: ProjectViewResourceType;
  locator: {
    locatorType: ProjectViewLocatorType;
    value: string;
  };
  description: string;
};

type DataByType = {
  project_profile: ProjectProfileData;
  goal: ProjectGoalData;
  role: ProjectRoleData;
  plan: ProjectPlanData;
  stage: ProjectStageData;
  requirement: ProjectRequirementData;
  issue: ProjectIssueData;
  work: ProjectWorkData;
  resource: ProjectResourceData;
};

export type ProjectViewObjectRef = {
  objectType: ProjectViewObjectType;
  objectId: string;
};

export type ProjectViewRelations = {
  underGoalId?: string;
  underPlanId?: string;
  plannedInStageId?: string;
  about?: ProjectViewObjectRef;
  handles?: ProjectViewObjectRef;
};

export type ProjectViewObjectOf<T extends ProjectViewObjectType> = {
  id: string;
  objectType: T;
  objectRevision: number;
  projectRevision: number;
  createdAt: string;
  updatedAt: string;
  createdBy: string;
  updatedBy: string;
  data: DataByType[T];
  relations: ProjectViewRelations;
};

export type ProjectViewObject = {
  [T in ProjectViewObjectType]: ProjectViewObjectOf<T>;
}[ProjectViewObjectType];

export type ProjectViewPlan = {
  plan: ProjectViewObjectOf<"plan">;
  stages: ProjectViewStage[];
};

export type ProjectViewStage = {
  stage: ProjectViewObjectOf<"stage">;
  requirements: ProjectViewRequirement[];
  issues: ProjectViewIssue[];
};

export type ProjectViewRequirement = {
  requirement: ProjectViewObjectOf<"requirement">;
  works: Array<ProjectViewObjectOf<"work">>;
};

export type ProjectViewIssue = {
  issue: ProjectViewObjectOf<"issue">;
  works: Array<ProjectViewObjectOf<"work">>;
};

export type ProjectView = {
  profile: ProjectViewObjectOf<"project_profile">;
  goals: Array<{
    goal: ProjectViewObjectOf<"goal">;
    plans: ProjectViewPlan[];
  }>;
  unboundPlans: ProjectViewPlan[];
  unplannedRequirements: ProjectViewRequirement[];
  unplannedIssues: ProjectViewIssue[];
  roles: Array<ProjectViewObjectOf<"role">>;
  resources: Array<ProjectViewObjectOf<"resource">>;
  issueReferencesByTarget: Record<string, ProjectViewObjectRef[]>;
};

export type ProjectViewLoadResult =
  | { status: "unsupported" }
  | { status: "forbidden" }
  | { status: "uninitialized"; relayPubkey: string }
  | {
      status: "ready";
      relayPubkey: string;
      projectRevision: number;
      projectionGeneration: number;
      activeObjectCount: number;
      updatedAt: string;
      view: ProjectView;
    };

export type ProjectViewWritableObject =
  | { objectType: "project_profile"; data: ProjectProfileData }
  | { objectType: "goal"; data: ProjectGoalData }
  | { objectType: "role"; data: ProjectRoleData }
  | {
      objectType: "plan";
      data: ProjectPlanData;
      underGoalId?: string;
    }
  | {
      objectType: "stage";
      data: ProjectStageData;
      underPlanId: string;
    }
  | {
      objectType: "requirement";
      data: ProjectRequirementData;
      plannedInStageId?: string;
    }
  | {
      objectType: "issue";
      data: ProjectIssueData;
      plannedInStageId?: string;
      about?: ProjectViewObjectRef;
    }
  | {
      objectType: "work";
      data: ProjectWorkData;
      handles: ProjectViewObjectRef;
    }
  | { objectType: "resource"; data: ProjectResourceData };

export type ProjectViewMutationIntent =
  | {
      operation: "initialize";
      profile: ProjectProfileData;
      goals: ProjectGoalData[];
    }
  | {
      operation: "create";
      expectedProjectRevision: number;
      object: Exclude<
        ProjectViewWritableObject,
        { objectType: "project_profile" }
      >;
    }
  | {
      operation: "update";
      expectedProjectRevision: number;
      objectId: string;
      object: ProjectViewWritableObject;
    }
  | {
      operation: "delete";
      expectedProjectRevision: number;
      objectType: Exclude<ProjectViewObjectType, "project_profile">;
      objectId: string;
    };

export type RawProjectViewMutationResult =
  | {
      status: "applied";
      event_id: string;
      project_revision: number;
      object_id?: string;
      object_revision?: number;
      deleted?: boolean;
    }
  | {
      status: "conflict";
      expected_project_revision: number;
      current_project_revision?: number;
      message: string;
    };

export type ProjectViewMutationResult =
  | {
      status: "applied";
      eventId: string;
      projectRevision: number;
      objectId?: string;
      objectRevision?: number;
      deleted?: boolean;
    }
  | {
      status: "conflict";
      expectedProjectRevision: number;
      currentProjectRevision?: number;
      message: string;
    };

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

function commonObjectFields<T extends ProjectViewObjectType>(
  raw: RawProjectViewObjectOf<T>,
) {
  if (raw.data.object_type !== raw.object_type) {
    throw new Error(
      "Project View integrity error: object type does not match its data",
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
  };
}

export function normalizeProjectViewObject(
  raw: RawProjectViewObject,
): ProjectViewObject {
  switch (raw.object_type) {
    case "project_profile":
      return {
        ...commonObjectFields(raw),
        objectType: raw.object_type,
        data: raw.data.data,
      };
    case "goal":
      return {
        ...commonObjectFields(raw),
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
        ...commonObjectFields(raw),
        objectType: raw.object_type,
        data: raw.data.data,
      } as ProjectViewObject;
    case "resource":
      return {
        ...commonObjectFields(raw),
        objectType: raw.object_type,
        data: {
          name: raw.data.data.name,
          resourceType: raw.data.data.resource_type,
          locator: {
            locatorType: raw.data.data.locator.locator_type,
            value: raw.data.data.locator.value,
          },
          description: raw.data.data.description,
        },
      };
  }
}

function normalizePlan(raw: RawPlanView): ProjectViewPlan {
  return {
    plan: normalizeProjectViewObject(raw.plan) as ProjectViewObjectOf<"plan">,
    stages: raw.stages.map(normalizeStage),
  };
}

function normalizeStage(raw: RawStageView): ProjectViewStage {
  return {
    stage: normalizeProjectViewObject(
      raw.stage,
    ) as ProjectViewObjectOf<"stage">,
    requirements: raw.requirements.map(normalizeRequirement),
    issues: raw.issues.map(normalizeIssue),
  };
}

function normalizeRequirement(raw: RawRequirementView): ProjectViewRequirement {
  return {
    requirement: normalizeProjectViewObject(
      raw.requirement,
    ) as ProjectViewObjectOf<"requirement">,
    works: raw.works.map(
      (work) => normalizeProjectViewObject(work) as ProjectViewObjectOf<"work">,
    ),
  };
}

function normalizeIssue(raw: RawIssueView): ProjectViewIssue {
  return {
    issue: normalizeProjectViewObject(
      raw.issue,
    ) as ProjectViewObjectOf<"issue">,
    works: raw.works.map(
      (work) => normalizeProjectViewObject(work) as ProjectViewObjectOf<"work">,
    ),
  };
}

export function normalizeProjectView(raw: RawProjectView): ProjectView {
  return {
    profile: normalizeProjectViewObject(
      raw.profile,
    ) as ProjectViewObjectOf<"project_profile">,
    goals: raw.goals.map(({ goal, plans }) => ({
      goal: normalizeProjectViewObject(goal) as ProjectViewObjectOf<"goal">,
      plans: plans.map(normalizePlan),
    })),
    unboundPlans: raw.unbound_plans.map(normalizePlan),
    unplannedRequirements: raw.unplanned_requirements.map(normalizeRequirement),
    unplannedIssues: raw.unplanned_issues.map(normalizeIssue),
    roles: raw.roles.map(
      (role) => normalizeProjectViewObject(role) as ProjectViewObjectOf<"role">,
    ),
    resources: raw.resources.map(
      (resource) =>
        normalizeProjectViewObject(resource) as ProjectViewObjectOf<"resource">,
    ),
    issueReferencesByTarget: Object.fromEntries(
      Object.entries(raw.issue_references_by_target).map(([target, refs]) => [
        target,
        refs.map(normalizeRef),
      ]),
    ),
  };
}

export function normalizeProjectViewLoadResult(
  raw: RawProjectViewLoadResult,
): ProjectViewLoadResult {
  switch (raw.status) {
    case "unsupported":
    case "forbidden":
      return raw;
    case "uninitialized":
      return {
        status: raw.status,
        relayPubkey: raw.relay_pubkey,
      };
    case "ready":
      return {
        status: raw.status,
        relayPubkey: raw.relay_pubkey,
        projectRevision: raw.project_revision,
        projectionGeneration: raw.projection_generation,
        activeObjectCount: raw.active_object_count,
        updatedAt: raw.updated_at,
        view: normalizeProjectView(raw.view),
      };
  }
}

export async function getProjectView(): Promise<ProjectViewLoadResult> {
  const raw = await invokeTauri<RawProjectViewLoadResult>("get_project_view");
  return normalizeProjectViewLoadResult(raw);
}

function rawReference(reference: ProjectViewObjectRef) {
  return {
    object_type: reference.objectType,
    object_id: reference.objectId,
  };
}

function serializeWritableObject(
  object: ProjectViewWritableObject,
): Record<string, unknown> {
  switch (object.objectType) {
    case "project_profile":
      return object.data;
    case "goal":
      return {
        title: object.data.title,
        desired_outcome: object.data.desiredOutcome,
        directions: object.data.directions,
      };
    case "role":
      return object.data;
    case "plan":
      return {
        ...object.data,
        under_goal_id: object.underGoalId ?? null,
      };
    case "stage":
      return {
        ...object.data,
        under_plan_id: object.underPlanId,
      };
    case "requirement":
      return {
        ...object.data,
        planned_in_stage_id: object.plannedInStageId ?? null,
      };
    case "issue":
      return {
        ...object.data,
        planned_in_stage_id: object.plannedInStageId ?? null,
        about: object.about ? rawReference(object.about) : null,
      };
    case "work":
      return {
        ...object.data,
        handles: rawReference(object.handles),
      };
    case "resource":
      return {
        name: object.data.name,
        resource_type: object.data.resourceType,
        locator: {
          locator_type: object.data.locator.locatorType,
          value: object.data.locator.value,
        },
        description: object.data.description,
      };
  }
}

export function serializeProjectViewMutationIntent(
  intent: ProjectViewMutationIntent,
): Record<string, unknown> {
  switch (intent.operation) {
    case "initialize":
      return {
        operation: intent.operation,
        profile: intent.profile,
        goals: intent.goals.map((goal) => ({
          title: goal.title,
          desired_outcome: goal.desiredOutcome,
          directions: goal.directions,
        })),
      };
    case "create":
      return {
        operation: intent.operation,
        expected_project_revision: intent.expectedProjectRevision,
        object_type: intent.object.objectType,
        data: serializeWritableObject(intent.object),
      };
    case "update":
      return {
        operation: intent.operation,
        expected_project_revision: intent.expectedProjectRevision,
        object_type: intent.object.objectType,
        object_id: intent.objectId,
        patch: serializeWritableObject(intent.object),
      };
    case "delete":
      return {
        operation: intent.operation,
        expected_project_revision: intent.expectedProjectRevision,
        object_type: intent.objectType,
        object_id: intent.objectId,
      };
  }
}

function normalizeMutationResult(
  raw: RawProjectViewMutationResult,
): ProjectViewMutationResult {
  switch (raw.status) {
    case "applied":
      return {
        status: raw.status,
        eventId: raw.event_id,
        projectRevision: raw.project_revision,
        objectId: raw.object_id,
        objectRevision: raw.object_revision,
        deleted: raw.deleted,
      };
    case "conflict":
      return {
        status: raw.status,
        expectedProjectRevision: raw.expected_project_revision,
        currentProjectRevision: raw.current_project_revision,
        message: raw.message,
      };
  }
}

export async function mutateProjectView(
  intent: ProjectViewMutationIntent,
): Promise<ProjectViewMutationResult> {
  const raw = await invokeTauri<RawProjectViewMutationResult>(
    "mutate_project_view",
    { input: serializeProjectViewMutationIntent(intent) },
  );
  return normalizeMutationResult(raw);
}
