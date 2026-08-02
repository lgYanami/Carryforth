import { invokeTauri } from "@/shared/api/tauri";
import { canonicalizeProjectViewContextReferences } from "@/shared/api/tauriProjectViewContext";
import {
  isProjectResourceDataV3,
  type ProjectViewMutationIntent,
  type ProjectViewMutationResult,
  type ProjectViewContextReference,
  type ProjectViewObjectRef,
  type ProjectViewWritableObject,
  type RawProjectViewMutationResult,
} from "@/shared/api/tauriProjectView";

function rawReference(reference: ProjectViewObjectRef) {
  return {
    object_type: reference.objectType,
    object_id: reference.objectId,
  };
}

function rawContextReference(reference: ProjectViewContextReference) {
  return reference.referenceType === "resource"
    ? { type: "resource", resource_id: reference.resourceId }
    : {
        type: "document",
        document_id: reference.documentId,
        mode: reference.mode,
        ...(reference.mode === "pinned"
          ? { document_revision: reference.documentRevision }
          : {}),
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
      return isProjectResourceDataV3(object.data)
        ? {
            name: object.data.name,
            resource_kind: object.data.resourceKind,
            summary: object.data.summary,
            guide_document_id: object.data.guideDocumentId,
          }
        : {
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
    case "context":
      return {
        operation: intent.operation,
        expected_project_revision: intent.expectedProjectRevision,
        object_type: intent.objectType,
        object_id: intent.objectId,
        context_references: canonicalizeProjectViewContextReferences(
          intent.contextReferences,
        ).map(rawContextReference),
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
