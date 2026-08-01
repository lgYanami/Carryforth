import type {
  ProjectViewLocatorType,
  ProjectViewPriority,
  ProjectViewResourceType,
} from "@/shared/api/tauriProjectView";

export const CREATE_GUIDE_VALUE = "__create_guide__";

export type ProjectViewObjectFormState = {
  name: string;
  title: string;
  positioning: string;
  purpose: string;
  problem: string;
  scope: string;
  description: string;
  desiredOutcome: string;
  directions: string;
  responsibilities: string;
  boundaries: string;
  active: boolean;
  status: string;
  priority: ProjectViewPriority;
  underGoalId: string;
  underPlanId: string;
  plannedInStageId: string;
  aboutId: string;
  handlesId: string;
  resourceType: ProjectViewResourceType;
  locatorType: ProjectViewLocatorType;
  locatorValue: string;
  resourceKind: string;
  summary: string;
  guideDocumentId: string;
  guideTitle: string;
  guideSummary: string;
  guideContentMarkdown: string;
};
