import { expect, test } from "@playwright/test";

import type {
  ProjectContextErrorPayload,
  ProjectContextQueryResult,
} from "../../src/shared/api/tauriProjectContext";
import { installMockBridge } from "../helpers/bridge";

const PROJECT_ID = "10000000-0000-4000-8000-000000000001";
const REQUIREMENT_ID = "20000000-0000-4000-8000-000000000001";
const RESOURCE_ID = "30000000-0000-4000-8000-000000000001";
const DOCUMENT_COORDINATE_ID = "40000000-0000-4000-8000-000000000001";
const CONTEXT_DOCUMENT_A_ID = "40000000-0000-4000-8000-000000000002";
const CONTEXT_DOCUMENT_B_ID = "40000000-0000-4000-8000-000000000003";
const RELAY = "b".repeat(64);
const ACTOR = "a".repeat(64);
const COMMUNITY_A = {
  id: "context-a",
  name: "Alpha",
  relayUrl: "ws://localhost:3000",
  addedAt: "2026-08-06T00:00:00.000Z",
};
const COMMUNITY_B = {
  id: "context-b",
  name: "Bravo",
  relayUrl: "ws://localhost:3001",
  addedAt: "2026-08-06T00:01:00.000Z",
};

function contextResult(input?: {
  capabilityEnabled?: boolean;
  edgeCount?: 0 | 1 | 2;
  revision?: number;
}): ProjectContextQueryResult {
  const edgeCount = input?.edgeCount ?? 1;
  const edges = [
    {
      edgeKey: "1".repeat(64),
      coordinateKeys: [
        `requirement:${REQUIREMENT_ID}`,
        `resource:${RESOURCE_ID}`,
      ],
      contextDocumentIds: [CONTEXT_DOCUMENT_A_ID],
    },
    {
      edgeKey: "2".repeat(64),
      coordinateKeys: [
        `requirement:${REQUIREMENT_ID}`,
        `document:${DOCUMENT_COORDINATE_ID}`,
      ],
      contextDocumentIds: [CONTEXT_DOCUMENT_B_ID],
    },
  ].slice(0, edgeCount);
  return {
    communityKey: "fixture",
    projectId: PROJECT_ID,
    relayPubkey: RELAY,
    context: {
      contextRevision: input?.revision ?? 7,
      projectionGeneration: 2,
      activeEdgeCount: edgeCount,
      boundDocumentCount: edgeCount,
      updatedAt: "2026-08-06T08:00:00Z",
      metaEventId: "c".repeat(64),
      capabilityEnabled: input?.capabilityEnabled ?? true,
    },
    query: { type: "contains_all", coordinates: [] },
    projectViewObservation: {
      state: "observed",
      projectRevision: 11,
      projectionGeneration: 3,
      updatedAt: "2026-08-06T08:00:00Z",
      metaEventId: "d".repeat(64),
    },
    documentObservation: {
      state: "observed",
      catalogRevision: 5,
      projectionGeneration: 2,
      updatedAt: "2026-08-06T08:00:00Z",
      metaEventId: "e".repeat(64),
    },
    edges,
    coordinateDetails: edgeCount
      ? [
          {
            coordinateKey: `requirement:${REQUIREMENT_ID}`,
            coordinate: {
              type: "project_view_object",
              objectType: "requirement",
              objectId: REQUIREMENT_ID,
            },
            state: "active",
            title: "Verified requirement",
            objectRevision: 4,
            updatedAt: "2026-08-06T08:00:00Z",
            updatedBy: ACTOR,
          },
          {
            coordinateKey: `resource:${RESOURCE_ID}`,
            coordinate: {
              type: "project_view_object",
              objectType: "resource",
              objectId: RESOURCE_ID,
            },
            state: "active",
            title: "Verified resource",
            objectRevision: 2,
            updatedAt: "2026-08-06T08:00:00Z",
            updatedBy: ACTOR,
          },
          ...(edgeCount > 1
            ? [
                {
                  coordinateKey: `document:${DOCUMENT_COORDINATE_ID}`,
                  coordinate: {
                    type: "document" as const,
                    documentId: DOCUMENT_COORDINATE_ID,
                  },
                  state: "active" as const,
                  title: "Supporting design document",
                  documentRevision: 6,
                  updatedAt: "2026-08-06T08:00:00Z",
                  updatedBy: ACTOR,
                },
              ]
            : []),
        ]
      : [],
    documentDetails: edgeCount
      ? [
          {
            documentId: CONTEXT_DOCUMENT_A_ID,
            state: "active",
            title: "Context rationale",
            summary: "Why these coordinates belong together.",
            documentRevision: 3,
            updatedAt: "2026-08-06T08:00:00Z",
            updatedBy: ACTOR,
          },
          ...(edgeCount > 1
            ? [
                {
                  documentId: CONTEXT_DOCUMENT_B_ID,
                  state: "active" as const,
                  title: "Additional Context rationale",
                  summary: "Why the document participates in this Context.",
                  documentRevision: 2,
                  updatedAt: "2026-08-06T08:00:00Z",
                  updatedBy: ACTOR,
                },
              ]
            : []),
        ]
      : [],
  };
}

async function openProjectContext(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByTestId("open-project-context").click();
  await expect(page).toHaveURL(/#\/project-context$/);
}

async function seedCommunities(page: import("@playwright/test").Page) {
  await page.addInitScript(
    ({ active, communities }) => {
      window.localStorage.setItem(
        "buzz-communities",
        JSON.stringify(communities),
      );
      window.localStorage.setItem("buzz-active-community-id", active);
    },
    { active: COMMUNITY_A.id, communities: [COMMUNITY_A, COMMUNITY_B] },
  );
}

test("sidebar order, active route, and default All query reach the trusted graph slot", async ({
  page,
}) => {
  await installMockBridge(page, { projectContext: contextResult() });
  await openProjectContext(page);

  const overview = page.getByTestId("open-community-overview");
  const context = page.getByTestId("open-project-context");
  const documents = page.getByTestId("open-documents");
  const [overviewBox, contextBox, documentsBox] = await Promise.all([
    overview.boundingBox(),
    context.boundingBox(),
    documents.boundingBox(),
  ]);
  expect(overviewBox?.y).toBeLessThan(contextBox?.y ?? 0);
  expect(contextBox?.y).toBeLessThan(documentsBox?.y ?? 0);
  await expect(context).toHaveAttribute("data-active", "true");
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "1");

  const calls = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__,
  );
  expect(calls).toHaveLength(1);
  expect(calls?.[0]?.payload).toMatchObject({
    input: {
      query: { type: "contains_all", coordinates: [] },
    },
  });
});

test("direct route shows an initialized empty catalog without claiming a Gap", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectContext: contextResult({ edgeCount: 0 }),
  });
  await page.goto("/#/project-context");
  await expect(page.getByTestId("project-context-empty")).toBeVisible();
  await expect(page.getByText("No Context Edges recorded yet")).toBeVisible();
  await expect(page.getByText(/Context Gap/i)).toHaveCount(0);
});

const failureCases: Array<{
  error: ProjectContextErrorPayload;
  testId: string;
  title: string;
}> = [
  {
    error: {
      code: "unsupported",
      message: "Project View v3 is required.",
      retryable: false,
    },
    testId: "project-context-unsupported",
    title: "Project Context is not supported",
  },
  {
    error: {
      code: "restricted",
      message: "Current Community membership is required.",
      retryable: false,
      status: 403,
    },
    testId: "project-context-restricted",
    title: "Project Context access denied",
  },
  {
    error: {
      code: "unavailable",
      message: "No verified projection is available.",
      retryable: true,
    },
    testId: "project-context-unavailable",
    title: "Project Context is not available yet",
  },
  {
    error: {
      code: "snapshot_conflict",
      message: "Context changed during the read.",
      retryable: true,
      status: 409,
    },
    testId: "project-context-snapshot-conflict",
    title: "Project Context changed while loading",
  },
  {
    error: {
      code: "verification_failed",
      message: "The Edge projection was inconsistent.",
      retryable: false,
    },
    testId: "project-context-verification-failed",
    title: "Project Context verification failed",
  },
];

for (const failure of failureCases) {
  test(`${failure.error.code} has a distinct fail-closed page state`, async ({
    page,
  }) => {
    await installMockBridge(page, { projectContextReadError: failure.error });
    await page.goto("/#/project-context");
    await expect(page.getByTestId(failure.testId)).toBeVisible();
    await expect(
      page.getByRole("heading", { name: failure.title }),
    ).toBeVisible();
    await expect(page.getByTestId("project-context-graph-slot")).toHaveCount(0);
  });
}

test("capability-off verified projection remains explicitly read-only", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectContext: contextResult({ capabilityEnabled: false }),
  });
  await page.goto("/#/project-context");
  await expect(page.getByText("Capability off · read-only")).toBeVisible();
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
  await expect(page.getByTestId("project-context-unsupported")).toHaveCount(0);
});

test("refresh failure keeps the same verified result and marks it stale", async ({
  page,
}) => {
  await installMockBridge(page, { projectContext: contextResult() });
  await page.goto("/#/project-context");
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
  await page.evaluate(() => {
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT_ERROR__?.({
      code: "unavailable",
      message: "Relay temporarily unavailable.",
      retryable: true,
    });
  });
  await page.getByTestId("project-context-refresh").click();
  await expect(page.getByTestId("project-context-stale-message")).toContainText(
    "Relay temporarily unavailable",
  );
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
});

test("Community switch never paints the previous Project Context result", async ({
  page,
}) => {
  await seedCommunities(page);
  await installMockBridge(
    page,
    {
      projectContextsByRelayUrl: {
        [COMMUNITY_A.relayUrl]: contextResult({ edgeCount: 1, revision: 7 }),
        [COMMUNITY_B.relayUrl]: contextResult({ edgeCount: 2, revision: 12 }),
      },
      projectContextReadDelayMsByRelayUrl: {
        [COMMUNITY_B.relayUrl]: 250,
      },
    },
    { skipCommunitySeed: true },
  );
  await openProjectContext(page);
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "1");

  await page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`).click();
  await page.getByTestId("open-project-context").click();
  await expect(page.getByTestId("project-context-loading")).toBeVisible();
  await expect(page.getByTestId("project-context-result-counts")).toHaveCount(
    0,
  );
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "2");
});
