import { invokeTauri } from "@/shared/api/tauri";
import { canonicalizeProjectViewContextReferences } from "@/shared/api/tauriProjectViewContext";
import type {
  ProjectViewMutationIntent,
  ProjectViewMutationResult,
  ProjectViewContextReference,
  ProjectViewObjectRef,
  ProjectViewWritableObject,
  RawProjectViewMutationResult,
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

function withOptionalSummary(
  data: Record<string, unknown> & { summary?: string },
): Record<string, unknown> {
  const { summary, ...rest } = data;
  return summary === undefined ? rest : { ...rest, summary };
}

function serializeWritableObject(
  object: ProjectViewWritableObject,
): Record<string, unknown> {
  switch (object.objectType) {
    case "project_profile":
      return withOptionalSummary(object.data);
    case "goal":
      return {
        title: object.data.title,
        ...(object.data.summary === undefined
          ? {}
          : { summary: object.data.summary }),
        desired_outcome: object.data.desiredOutcome,
        directions: object.data.directions,
      };
    case "role":
      return withOptionalSummary(object.data);
    case "plan":
      return {
        ...withOptionalSummary(object.data),
        under_goal_id: object.underGoalId ?? null,
      };
    case "stage":
      return {
        ...withOptionalSummary(object.data),
        under_plan_id: object.underPlanId,
      };
    case "requirement":
      return {
        ...withOptionalSummary(object.data),
        planned_in_stage_id: object.plannedInStageId ?? null,
      };
    case "issue":
      return {
        ...withOptionalSummary(object.data),
        planned_in_stage_id: object.plannedInStageId ?? null,
        about: object.about ? rawReference(object.about) : null,
      };
    case "work":
      return {
        ...withOptionalSummary(object.data),
        handles: rawReference(object.handles),
      };
    case "resource":
      return {
        name: object.data.name,
        resource_kind: object.data.resourceKind,
        ...(object.data.summary === undefined
          ? {}
          : { summary: object.data.summary }),
        guide_document_id: object.data.guideDocumentId,
      };
  }
}

export function serializeProjectViewMutationIntent(
  intent: ProjectViewMutationIntent,
): Record<string, unknown> {
  switch (intent.operation) {
    case "create":
      return {
        operation: intent.operation,
        expected_project_revision: intent.expectedProjectRevision,
        object_type: intent.object.objectType,
        data: serializeWritableObject(intent.object),
        ...(intent.initialRoleLevel
          ? { initial_role_level: intent.initialRoleLevel }
          : {}),
        ...(intent.actingAssignmentId
          ? { acting_assignment_id: intent.actingAssignmentId }
          : {}),
      };
    case "update": {
      const patch = serializeWritableObject(intent.object);
      if (intent.summaryPatch === undefined) {
        delete patch.summary;
      } else {
        patch.summary = intent.summaryPatch;
      }
      return {
        operation: intent.operation,
        expected_project_revision: intent.expectedProjectRevision,
        object_type: intent.object.objectType,
        object_id: intent.objectId,
        patch,
        ...(intent.actingAssignmentId
          ? { acting_assignment_id: intent.actingAssignmentId }
          : {}),
      };
    }
    case "delete":
      return {
        operation: intent.operation,
        expected_project_revision: intent.expectedProjectRevision,
        object_type: intent.objectType,
        object_id: intent.objectId,
        ...(intent.actingAssignmentId
          ? { acting_assignment_id: intent.actingAssignmentId }
          : {}),
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
        ...(intent.actingAssignmentId
          ? { acting_assignment_id: intent.actingAssignmentId }
          : {}),
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
        confirmation: raw.confirmation ?? "current_verified",
        currentObjectRevision: raw.current_object_revision,
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
