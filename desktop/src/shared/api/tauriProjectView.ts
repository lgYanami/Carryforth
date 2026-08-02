import { invokeTauri } from "@/shared/api/tauri";
import { ProjectViewIntegrityError } from "@/shared/api/tauriProjectViewIntegrity";
import { assembleProjectViewV3 } from "@/shared/api/tauriProjectViewV3";
import {
  normalizeRoleContinuity,
  type ProjectViewRoleContinuity,
  type RawProjectViewRoleContinuity,
} from "@/shared/api/tauriProjectViewRole";

export {
  ProjectViewIntegrityError,
  isProjectViewIntegrityError,
} from "@/shared/api/tauriProjectViewIntegrity";
export {
  mutateProjectViewRole,
  serializeProjectViewRoleMutationIntent,
} from "@/shared/api/tauriProjectViewRole";
export {
  mutateProjectView,
  serializeProjectViewMutationIntent,
} from "@/shared/api/tauriProjectViewMutation";
export { canonicalizeProjectViewContextReferences } from "@/shared/api/tauriProjectViewContext";
export {
  assembleProjectViewV3,
  normalizeProjectViewObjectV3,
} from "@/shared/api/tauriProjectViewV3";
export type {
  ProjectCommunityMemberRole,
  ProjectRoleAssignment,
  ProjectRoleAssignmentEndReason,
  ProjectRoleBrief,
  ProjectRoleCheckpoint,
  ProjectRoleCheckpointContent,
  ProjectRoleContinuityReference,
  ProjectRoleDefinition,
  ProjectRoleHandoff,
  ProjectRoleHandoffCause,
  ProjectRoleHandoffContent,
  ProjectRoleLevel,
  ProjectRoleProposal,
  ProjectRoleProposalStatus,
  ProjectRoleProposalType,
  ProjectViewRoleMutationIntent,
  ProjectViewRoleMutationResult,
  ProjectViewRoleContinuity,
  RawProjectViewRoleMutationResult,
} from "@/shared/api/tauriProjectViewRole";

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

type RawResourceDataV3 = {
  name: string;
  resource_kind: string;
  summary?: string;
  guide_document_id: string;
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

type RawDataByTypeV3 = Omit<RawDataByType, "resource"> & {
  resource: RawResourceDataV3;
};

export type RawProjectViewContextReference =
  | { type: "resource"; resource_id: string }
  | {
      type: "document";
      document_id: string;
      mode: "live" | "pinned";
      document_revision?: number;
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

export type RawProjectViewObjectV3Of<T extends ProjectViewObjectType> = {
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
    data: RawDataByTypeV3[T];
  };
  relations: RawProjectViewRelations;
  context_references: RawProjectViewContextReference[];
};

export type RawProjectViewObjectV3 = {
  [T in ProjectViewObjectType]: RawProjectViewObjectV3Of<T>;
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
      project_context_supported?: boolean;
      schema_version: 1 | 2;
      project_revision: number;
      projection_generation: number;
      active_object_count: number;
      updated_at: string;
      view: RawProjectView;
      objects_v3?: never;
      role_continuity?: RawProjectViewRoleContinuity;
    }
  | {
      status: "ready";
      relay_pubkey: string;
      project_context_supported?: boolean;
      schema_version: 3;
      project_revision: number;
      projection_generation: number;
      active_object_count: number;
      updated_at: string;
      view?: never;
      objects_v3: RawProjectViewObjectV3[];
      role_continuity: RawProjectViewRoleContinuity;
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
export type ProjectResourceDataLegacy = {
  name: string;
  resourceType: ProjectViewResourceType;
  locator: {
    locatorType: ProjectViewLocatorType;
    value: string;
  };
  description: string;
};

export type ProjectResourceDataV3 = {
  name: string;
  resourceKind: string;
  summary?: string;
  guideDocumentId: string;
};

export type ProjectResourceData =
  | ProjectResourceDataLegacy
  | ProjectResourceDataV3;

export function isProjectResourceDataV3(
  resource: ProjectResourceData,
): resource is ProjectResourceDataV3 {
  return "guideDocumentId" in resource;
}

export type ProjectViewContextReference =
  | { referenceType: "resource"; resourceId: string }
  | {
      referenceType: "document";
      documentId: string;
      mode: "live" | "pinned";
      documentRevision?: number;
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
  contextReferences?: ProjectViewContextReference[];
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
      contextCapability: boolean;
      schemaVersion: 1 | 2 | 3;
      projectRevision: number;
      projectionGeneration: number;
      activeObjectCount: number;
      updatedAt: string;
      view: ProjectView;
      roleContinuity?: ProjectViewRoleContinuity;
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
    }
  | {
      operation: "context";
      expectedProjectRevision: number;
      objectType: ProjectViewObjectType;
      objectId: string;
      contextReferences: ProjectViewContextReference[];
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
    throw new ProjectViewIntegrityError("object type does not match its data");
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

function assertProjectViewSnapshotIntegrity(
  view: ProjectView,
  projectRevision: number,
  activeObjectCount: number,
): void {
  if (!Number.isSafeInteger(projectRevision) || projectRevision < 1) {
    throw new ProjectViewIntegrityError(
      "the verified project revision is invalid",
    );
  }
  if (view.goals.length === 0) {
    throw new ProjectViewIntegrityError(
      "an initialized View must contain at least one Goal",
    );
  }

  const objects = new Map<string, ProjectViewObject>();
  const register = (object: ProjectViewObject, location: string) => {
    if (objects.has(object.id)) {
      throw new ProjectViewIntegrityError(
        `object ${object.id} appears more than once`,
      );
    }
    if (
      !Number.isSafeInteger(object.objectRevision) ||
      object.objectRevision < 1
    ) {
      throw new ProjectViewIntegrityError(
        `${location} has an invalid object revision`,
      );
    }
    if (
      !Number.isSafeInteger(object.projectRevision) ||
      object.projectRevision < 1 ||
      object.projectRevision > projectRevision
    ) {
      throw new ProjectViewIntegrityError(
        `${location} belongs to an impossible project revision`,
      );
    }
    objects.set(object.id, object);
  };
  const registerWork = (
    work: ProjectViewObjectOf<"work">,
    parent: ProjectViewObjectOf<"requirement"> | ProjectViewObjectOf<"issue">,
  ) => {
    register(work, `Work ${work.id}`);
    if (
      work.relations.handles?.objectId !== parent.id ||
      work.relations.handles.objectType !== parent.objectType
    ) {
      throw new ProjectViewIntegrityError(
        `Work ${work.id} is not placed under its Handles target`,
      );
    }
  };
  const registerRequirement = (
    entry: ProjectViewRequirement,
    stageId?: string,
  ) => {
    register(entry.requirement, `Requirement ${entry.requirement.id}`);
    if (entry.requirement.relations.plannedInStageId !== stageId) {
      throw new ProjectViewIntegrityError(
        `Requirement ${entry.requirement.id} is in the wrong Stage`,
      );
    }
    for (const work of entry.works) {
      registerWork(work, entry.requirement);
    }
  };
  const registerIssue = (entry: ProjectViewIssue, stageId?: string) => {
    register(entry.issue, `Issue ${entry.issue.id}`);
    if (entry.issue.relations.plannedInStageId !== stageId) {
      throw new ProjectViewIntegrityError(
        `Issue ${entry.issue.id} is in the wrong Stage`,
      );
    }
    for (const work of entry.works) {
      registerWork(work, entry.issue);
    }
  };
  const registerPlan = (entry: ProjectViewPlan, goalId?: string) => {
    register(entry.plan, `Plan ${entry.plan.id}`);
    if (entry.plan.relations.underGoalId !== goalId) {
      throw new ProjectViewIntegrityError(
        `Plan ${entry.plan.id} is under the wrong Goal`,
      );
    }
    for (const stage of entry.stages) {
      register(stage.stage, `Stage ${stage.stage.id}`);
      if (stage.stage.relations.underPlanId !== entry.plan.id) {
        throw new ProjectViewIntegrityError(
          `Stage ${stage.stage.id} is under the wrong Plan`,
        );
      }
      for (const requirement of stage.requirements) {
        registerRequirement(requirement, stage.stage.id);
      }
      for (const issue of stage.issues) {
        registerIssue(issue, stage.stage.id);
      }
    }
  };

  register(view.profile, "Project Profile");
  for (const { goal, plans } of view.goals) {
    register(goal, `Goal ${goal.id}`);
    for (const plan of plans) {
      registerPlan(plan, goal.id);
    }
  }
  for (const plan of view.unboundPlans) {
    registerPlan(plan);
  }
  for (const requirement of view.unplannedRequirements) {
    registerRequirement(requirement);
  }
  for (const issue of view.unplannedIssues) {
    registerIssue(issue);
  }
  for (const role of view.roles) {
    register(role, `Role ${role.id}`);
  }
  for (const resource of view.resources) {
    register(resource, `Resource ${resource.id}`);
  }

  if (objects.size !== activeObjectCount) {
    throw new ProjectViewIntegrityError(
      `active object count ${activeObjectCount} does not match the ${objects.size} assembled objects`,
    );
  }
  for (const [targetId, references] of Object.entries(
    view.issueReferencesByTarget,
  )) {
    if (!objects.has(targetId)) {
      throw new ProjectViewIntegrityError(
        `issue reference target ${targetId} is not active`,
      );
    }
    for (const reference of references) {
      const source = objects.get(reference.objectId);
      if (source?.objectType !== "issue") {
        throw new ProjectViewIntegrityError(
          `issue reference source ${reference.objectId} is not an active Issue`,
        );
      }
    }
  }
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
    case "ready": {
      const view =
        raw.schema_version === 3
          ? assembleProjectViewV3(raw.objects_v3)
          : normalizeProjectView(raw.view);
      assertProjectViewSnapshotIntegrity(
        view,
        raw.project_revision,
        raw.active_object_count,
      );
      let roleContinuity: ProjectViewRoleContinuity | undefined;
      if (raw.role_continuity) {
        if (raw.schema_version === 1) {
          throw new ProjectViewIntegrityError(
            "schema-v1 Project View returned Role continuity",
          );
        }
        roleContinuity = normalizeRoleContinuity(
          raw.role_continuity,
          view,
          raw.project_revision,
          raw.projection_generation,
          raw.schema_version,
        );
      }
      if (
        ((raw.schema_version === 2 || raw.schema_version === 3) &&
          !roleContinuity) ||
        (raw.schema_version === 1 && Boolean(roleContinuity))
      ) {
        throw new ProjectViewIntegrityError(
          "Role continuity payload does not match the Project View schema",
        );
      }
      return {
        status: raw.status,
        relayPubkey: raw.relay_pubkey,
        contextCapability: Boolean(raw.project_context_supported),
        schemaVersion: raw.schema_version,
        projectRevision: raw.project_revision,
        projectionGeneration: raw.projection_generation,
        activeObjectCount: raw.active_object_count,
        updatedAt: raw.updated_at,
        view,
        roleContinuity,
      };
    }
  }
}

export async function getProjectView(): Promise<ProjectViewLoadResult> {
  const raw = await invokeTauri<RawProjectViewLoadResult>("get_project_view");
  return normalizeProjectViewLoadResult(raw);
}
