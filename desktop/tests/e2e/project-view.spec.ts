import { expect, test } from "@playwright/test";

import type {
  RawProjectViewLoadResult,
  RawProjectViewObject,
  ProjectViewObjectType,
} from "../../src/shared/api/tauriProjectView";
import { installMockBridge } from "../helpers/bridge";

const ACTOR = "a".repeat(64);
const NOW = "2026-07-27T08:00:00Z";
const IDS = {
  profile: "00000000-0000-4000-8000-000000000001",
  goal: "00000000-0000-4000-8000-000000000002",
  plan: "00000000-0000-4000-8000-000000000003",
  stage: "00000000-0000-4000-8000-000000000004",
  requirement: "00000000-0000-4000-8000-000000000005",
  issue: "00000000-0000-4000-8000-000000000006",
  work: "00000000-0000-4000-8000-000000000007",
  looseIssue: "00000000-0000-4000-8000-000000000008",
  role: "00000000-0000-4000-8000-000000000009",
  resource: "00000000-0000-4000-8000-000000000010",
} as const;

function object(
  objectType: ProjectViewObjectType,
  id: string,
  data: Record<string, unknown>,
  relations: Record<string, unknown> = {},
): RawProjectViewObject {
  return {
    id,
    object_type: objectType,
    object_revision: 1,
    project_revision: 7,
    created_at: NOW,
    updated_at: NOW,
    created_by: ACTOR,
    updated_by: ACTOR,
    data: { object_type: objectType, data },
    relations,
  } as RawProjectViewObject;
}

const profile = object("project_profile", IDS.profile, {
  name: "Lora",
  positioning: "A shared context layer for humans and agents.",
  purpose: "Keep project intent and execution connected.",
  problem: "Project context is fragmented across tools and conversations.",
  scope: "The canonical project model and its collaborative surfaces.",
});
const goal = object("goal", IDS.goal, {
  title: "Make project context legible",
  desired_outcome: "Humans and agents read the same verified project map.",
  directions: ["Preserve canonical relations", "Make gaps visible"],
});
const plan = object(
  "plan",
  IDS.plan,
  {
    title: "Deliver Project View",
    description: "Ship a trustworthy project-context surface in slices.",
    status: "active",
  },
  { under_goal_id: IDS.goal },
);
const stage = object(
  "stage",
  IDS.stage,
  {
    title: "Read-only client",
    description: "Render one consistent verified snapshot.",
    status: "active",
  },
  { under_plan_id: IDS.plan },
);
const requirement = object(
  "requirement",
  IDS.requirement,
  {
    title: "Verified snapshot",
    description: "Reject incomplete or wrongly signed projection state.",
    status: "in_progress",
    priority: "high",
  },
  { planned_in_stage_id: IDS.stage },
);
const issue = object(
  "issue",
  IDS.issue,
  {
    title: "Projects naming overlap",
    description: "Community projects and Git repositories use similar terms.",
    status: "open",
    priority: "high",
  },
  {
    planned_in_stage_id: IDS.stage,
    about: { object_type: "project_profile", object_id: IDS.profile },
  },
);
const work = object(
  "work",
  IDS.work,
  {
    title: "Add the View entry",
    description: "Expose /view without changing the existing Projects entry.",
    status: "in_progress",
    priority: "normal",
  },
  {
    handles: { object_type: "requirement", object_id: IDS.requirement },
  },
);
const looseIssue = object("issue", IDS.looseIssue, {
  title: "Unplanned feedback",
  description: "Keep valid issues visible before they are assigned to a stage.",
  status: "open",
  priority: "normal",
});
const role = object("role", IDS.role, {
  name: "Context steward",
  purpose: "Keep project intent coherent.",
  responsibilities: ["Review project structure"],
  boundaries: ["Does not grant Buzz authorization"],
  active: true,
});
const resource = object("resource", IDS.resource, {
  name: "Buzz repository",
  resource_type: "repository",
  locator: {
    locator_type: "url",
    value: "https://github.com/block/buzz",
  },
  description: "Source repository for the Buzz implementation.",
});

const READY_VIEW = {
  status: "ready",
  relay_pubkey: "b".repeat(64),
  project_revision: 7,
  projection_generation: 2,
  active_object_count: 10,
  updated_at: NOW,
  view: {
    profile,
    goals: [
      {
        goal,
        plans: [
          {
            plan,
            stages: [
              {
                stage,
                requirements: [{ requirement, works: [work] }],
                issues: [{ issue, works: [] }],
              },
            ],
          },
        ],
      },
    ],
    unbound_plans: [],
    unplanned_requirements: [],
    unplanned_issues: [{ issue: looseIssue, works: [] }],
    roles: [role],
    resources: [resource],
    issue_references_by_target: {
      [IDS.profile]: [{ object_type: "issue", object_id: IDS.issue }],
    },
  },
} as RawProjectViewLoadResult;

test("View renders the verified canonical map and object inspector", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: READY_VIEW });
  await page.goto("/");

  await expect(page.getByTestId("open-projects-view")).toContainText(
    "Projects",
  );
  await page.getByTestId("open-view").click();

  await expect(page).toHaveURL(/\/view$/);
  await expect(page.getByTestId("project-view-profile")).toContainText("Lora");
  await expect(page.getByTestId("project-view-map")).toContainText(
    "Make project context legible",
  );
  await expect(page.getByTestId("project-view-map")).toContainText(
    "Verified snapshot",
  );
  await expect(page.getByTestId("project-view-unplanned")).toContainText(
    "Unplanned feedback",
  );

  await page
    .getByRole("button", { name: "Inspect Issue Projects naming overlap" })
    .click();
  await expect(page).toHaveURL(new RegExp(`object=${IDS.issue}`));
  await expect(page.getByTestId("project-view-inspector")).toContainText(
    "Projects naming overlap",
  );
  await expect(page.getByTestId("project-view-inspector")).toContainText(
    "Verified projection",
  );

  await page.getByRole("button", { name: "Close inspector" }).click();
  await expect(page).toHaveURL(/\/view$/);
});

test("View distinguishes supported but uninitialized Communities", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: {
      status: "uninitialized",
      relay_pubkey: "b".repeat(64),
    },
  });
  await page.goto("/");
  await page.getByTestId("open-view").click();

  await expect(
    page.getByRole("heading", {
      name: "This View has not been initialized",
    }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: /initialize/i })).toHaveCount(
    0,
  );
});

for (const state of [
  {
    name: "unsupported",
    result: { status: "unsupported" } as const,
    heading: "View is not supported by this Relay",
  },
  {
    name: "forbidden",
    result: { status: "forbidden" } as const,
    heading: "View access denied",
  },
]) {
  test(`View presents the ${state.name} capability state`, async ({ page }) => {
    await installMockBridge(page, { projectView: state.result });
    await page.goto("/");
    await page.getByTestId("open-view").click();

    await expect(
      page.getByRole("heading", { name: state.heading }),
    ).toBeVisible();
  });
}
