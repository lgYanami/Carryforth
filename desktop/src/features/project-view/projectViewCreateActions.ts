import type { ProjectViewCreateContext } from "@/features/project-view/model";
import type {
  ProjectViewObject,
  ProjectViewObjectType,
} from "@/shared/api/tauriProjectView";

export type ProjectViewCreateAction = {
  id: string;
  label: string;
  relation: "structural" | "related";
  initialType: Exclude<ProjectViewObjectType, "project_profile">;
  context?: ProjectViewCreateContext;
};

/** Build explicit create intents for the current object without inferring extra relations. */
export function projectViewCreateActions(
  object: ProjectViewObject,
): ProjectViewCreateAction[] {
  const structural: ProjectViewCreateAction[] = (() => {
    switch (object.objectType) {
      case "project_profile":
        return [
          {
            id: "goal",
            label: "Add Goal",
            relation: "structural",
            initialType: "goal",
          },
          {
            id: "role",
            label: "Add Role",
            relation: "structural",
            initialType: "role",
          },
          {
            id: "resource",
            label: "Add Resource",
            relation: "structural",
            initialType: "resource",
          },
          {
            id: "unbound-plan",
            label: "Add unbound Plan",
            relation: "structural",
            initialType: "plan",
          },
          {
            id: "unplanned-requirement",
            label: "Add unplanned Requirement",
            relation: "structural",
            initialType: "requirement",
          },
          {
            id: "unplanned-issue",
            label: "Add unplanned Issue",
            relation: "structural",
            initialType: "issue",
          },
        ];
      case "goal":
        return [
          {
            id: "plan",
            label: "Add Plan",
            relation: "structural",
            initialType: "plan",
            context: { underGoalId: object.id },
          },
        ];
      case "plan":
        return [
          {
            id: "stage",
            label: "Add Stage",
            relation: "structural",
            initialType: "stage",
            context: { underPlanId: object.id },
          },
        ];
      case "stage":
        return [
          {
            id: "requirement",
            label: "Add Requirement",
            relation: "structural",
            initialType: "requirement",
            context: { plannedInStageId: object.id },
          },
          {
            id: "issue",
            label: "Add planned Issue",
            relation: "structural",
            initialType: "issue",
            context: { plannedInStageId: object.id },
          },
        ];
      case "requirement":
      case "issue":
        return [
          {
            id: "work",
            label: "Add Work",
            relation: "structural",
            initialType: "work",
            context: {
              handles: {
                objectId: object.id,
                objectType: object.objectType,
              },
            },
          },
        ];
      case "work":
      case "role":
      case "resource":
        return [];
    }
  })();

  return [
    ...structural,
    {
      id: "related-issue",
      label: "Add related Issue",
      relation: "related",
      initialType: "issue",
      context: {
        about: { objectId: object.id, objectType: object.objectType },
      },
    },
  ];
}
