import { expect, test } from "@playwright/test";

import type {
  ProjectDocument,
  ProjectDocumentMeta,
} from "../../src/shared/api/tauriProjectDocument";
import type { ProjectContextQueryResult } from "../../src/shared/api/tauriProjectContext";
import type { MockProjectDocumentState } from "../../src/testing/e2eBridge";
import { waitForAnimations } from "../helpers/animations";
import { installMockBridge } from "../helpers/bridge";

const DOCUMENT_ID = "10000000-0000-4000-8000-000000000001";
const PROJECT_ID = "20000000-0000-4000-8000-000000000001";
const RELAY = "b".repeat(64);
const ACTOR = "a".repeat(64);
const COMMUNITY_A = {
  id: "documents-a",
  name: "Alpha",
  relayUrl: "ws://localhost:3000",
  addedAt: "2026-07-30T00:00:00.000Z",
};
const COMMUNITY_B = {
  id: "documents-b",
  name: "Bravo",
  relayUrl: "ws://localhost:3001",
  addedAt: "2026-07-30T00:01:00.000Z",
};

function revision(
  documentRevision: number,
  title: string,
  contentMarkdown: string,
): ProjectDocument {
  const at = `2026-07-30T08:0${documentRevision}:00Z`;
  return {
    communityKey: "fixture",
    projectId: PROJECT_ID,
    relayPubkey: RELAY,
    projectionGeneration: 2,
    documentId: DOCUMENT_ID,
    documentRevision,
    state: "active",
    title,
    summary: "A verified operating guide.",
    contentMarkdown,
    createdAt: "2026-07-30T08:01:00Z",
    createdBy: ACTOR,
    revisionAt: at,
    revisionBy: ACTOR,
    revisionEventId: documentRevision.toString().repeat(64).slice(0, 64),
    headEventId: "e".repeat(64),
    sourceEventId: "f".repeat(64),
  };
}

function documentState(
  title = "Release runbook",
  currentRevision = 2,
  currentContent = "# Release runbook\n\n```sh\njust ci\n```\n\n<script>window.__DOC_XSS__ = true</script>",
): MockProjectDocumentState {
  const first = revision(1, "Initial runbook", "# Initial\n\nCheck the build.");
  const current = revision(currentRevision, title, currentContent);
  const meta: ProjectDocumentMeta = {
    communityKey: "fixture",
    projectId: PROJECT_ID,
    relayPubkey: RELAY,
    projectionGeneration: 2,
    catalogRevision: currentRevision,
    activeDocumentCount: 1,
    updatedAt: current.revisionAt,
    metaEventId: "d".repeat(64),
  };
  return {
    meta,
    documents: [
      {
        documentId: DOCUMENT_ID,
        title,
        summary: current.summary,
        documentRevision: currentRevision,
        updatedAt: current.revisionAt,
        updatedBy: ACTOR,
        headEventId: current.headEventId ?? "e".repeat(64),
      },
    ],
    revisions: {
      [DOCUMENT_ID]: currentRevision === 1 ? [current] : [first, current],
    },
  };
}

function documentContextResult(): ProjectContextQueryResult {
  return {
    communityKey: "fixture",
    projectId: PROJECT_ID,
    relayPubkey: RELAY,
    context: {
      contextRevision: 1,
      projectionGeneration: 1,
      activeEdgeCount: 0,
      boundDocumentCount: 0,
      updatedAt: "2026-07-30T08:00:00Z",
      metaEventId: "c".repeat(64),
      capabilityEnabled: true,
    },
    query: { type: "contains_all", coordinates: [] },
    projectViewObservation: { state: "not_requested" },
    documentObservation: { state: "observed" },
    edges: [],
    coordinateDetails: [
      {
        coordinateKey: `document:${DOCUMENT_ID}`,
        coordinate: { type: "document", documentId: DOCUMENT_ID },
        state: "active",
        title: "Release runbook",
      },
    ],
    documentDetails: [],
  };
}

async function openDocuments(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByTestId("open-documents").click();
  await expect(page).toHaveURL(/\/documents/);
  await expect(page.getByTestId("document-list")).toBeVisible();
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

test("metadata-first list lazily reads safe Markdown and isolates pinned history", async ({
  page,
}) => {
  await installMockBridge(page, { projectDocument: documentState() });
  await openDocuments(page);

  await expect(page.getByText("Release runbook")).toBeVisible();
  const initialCalls = await page.evaluate(() =>
    window.__BUZZ_E2E_PROJECT_DOCUMENT_CALLS__?.map((call) => call.command),
  );
  expect(initialCalls).toContain("get_project_document_meta");
  expect(initialCalls).toContain("list_project_documents");
  expect(initialCalls).not.toContain("get_project_document");
  expect(initialCalls).not.toContain("get_project_document_history");

  await page.getByTestId(`document-list-item-${DOCUMENT_ID}`).click();
  await expect(page.getByTestId("document-markdown")).toContainText("just ci");
  await expect(
    page.getByTestId("document-markdown").locator("script"),
  ).toHaveCount(0);
  expect(await page.evaluate(() => "__DOC_XSS__" in window)).toBe(false);
  await waitForAnimations(page);
  await page.getByTestId("project-documents-screen").screenshot({
    path: "test-results/project-documents/01-reader.png",
  });

  await page.getByTestId("document-toggle-history").click();
  await page.getByTestId("document-history-r1").click();
  await expect(page.getByText("Initial runbook")).toBeVisible();
  await expect(page.getByText("Pinned r1")).toBeVisible();
  await expect(page.getByTestId("document-markdown")).toContainText(
    "Check the build",
  );
  await page.getByTestId("document-return-current").click();
  await expect(
    page
      .getByTestId("document-viewer")
      .locator("header")
      .getByRole("heading", { name: "Release runbook" }),
  ).toBeVisible();
  await expect(page.getByText("Current", { exact: true })).toBeVisible();
});

test("active Document opens Incident Project Context and browser Back returns", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectContext: documentContextResult(),
    projectDocument: documentState(),
  });
  await page.goto(`/#/documents?document=${DOCUMENT_ID}`);
  await expect(page.getByTestId("document-viewer")).toBeVisible();
  await page.getByTestId("document-show-in-project-context").click();

  await expect(page).toHaveURL(/project-context/);
  await expect(page).toHaveURL(/mode=incident/);
  await expect(page).toHaveURL(
    new RegExp(`coordinates=document(%3A|:)${DOCUMENT_ID}`),
  );
  await expect(page.getByTestId("project-context-query-summary")).toContainText(
    "0 matching edges",
  );
  const calls = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__,
  );
  expect(calls?.at(-1)?.payload).toMatchObject({
    input: {
      query: {
        type: "incident",
        coordinate: { type: "document", documentId: DOCUMENT_ID },
      },
    },
  });

  await page.goBack();
  await expect(page).toHaveURL(
    new RegExp(`/documents\\?document=${DOCUMENT_ID}$`),
  );
  await expect(page.getByTestId("document-viewer")).toBeVisible();
});

test("create, update, and tombstone stay on verified full snapshots", async ({
  page,
}) => {
  await installMockBridge(page, { projectDocument: documentState() });
  await openDocuments(page);
  await page.getByTestId("document-create").click();
  await expect(
    page.getByText("Documents are not a Secret Store"),
  ).toBeVisible();
  await page.getByTestId("document-title-input").fill("Incident notes");
  await page.getByTestId("document-summary-input").fill("Recovery record");
  await page
    .getByTestId("document-content-input")
    .fill("# Recovery\n\nAll clear.");
  await waitForAnimations(page);
  await page.getByTestId("project-documents-screen").screenshot({
    path: "test-results/project-documents/02-editor-warning.png",
  });
  await page.getByTestId("document-save").click();
  await expect(
    page.getByRole("heading", { name: "Incident notes" }),
  ).toBeVisible();

  await page.getByTestId("document-edit").click();
  await page
    .getByTestId("document-content-input")
    .fill("# Recovery\n\nReviewed.");
  await page.getByTestId("document-save").click();
  await expect(page.getByTestId("document-markdown")).toContainText("Reviewed");
  await page.getByTestId("document-delete").click();
  await page.getByTestId("document-delete-confirm").click();
  await expect(
    page.getByRole("heading", { name: "Incident notes" }),
  ).toHaveCount(0);

  const mutations = await page.evaluate(() =>
    window.__BUZZ_E2E_PROJECT_DOCUMENT_CALLS__?.filter(
      (call) => call.command === "mutate_project_document",
    ),
  );
  expect(mutations).toHaveLength(3);
});

test("revision conflict preserves local content and requires an explicit latest base", async ({
  page,
}) => {
  const initial = documentState();
  const latest = documentState(
    "Agent-updated runbook",
    3,
    "# Release runbook\n\nAgent changed this snapshot.",
  );
  latest.revisions[DOCUMENT_ID] = [
    ...initial.revisions[DOCUMENT_ID],
    latest.revisions[DOCUMENT_ID].at(-1) as ProjectDocument,
  ];
  await installMockBridge(page, { projectDocument: initial });
  await openDocuments(page);
  await page.getByTestId(`document-list-item-${DOCUMENT_ID}`).click();
  await page.getByTestId("document-edit").click();
  await page
    .getByTestId("document-content-input")
    .fill("# Release runbook\n\nHuman local draft must survive.");
  await page.evaluate((state) => {
    window.__BUZZ_E2E_SET_PROJECT_DOCUMENT_STATE__?.(state);
  }, latest);
  await page.getByTestId("document-save").click();

  await expect(page.getByTestId("document-conflict")).toContainText(
    "Your draft was preserved",
  );
  await expect(page.getByTestId("document-content-input")).toHaveValue(
    /Human local draft must survive/,
  );
  await page.getByRole("button", { name: "View diff" }).click();
  await expect(page.getByTestId("document-exact-diff").first()).toContainText(
    "Human local draft must survive",
  );
  await waitForAnimations(page);
  await page.getByTestId("project-documents-screen").screenshot({
    path: "test-results/project-documents/03-conflict-diff.png",
  });

  await page.getByRole("button", { name: "Edit on latest" }).click();
  await expect(page.getByTestId("document-content-input")).toHaveValue(
    /Human local draft must survive/,
  );
  await page.getByTestId("document-save").click();
  await expect(page.getByTestId("document-markdown")).toContainText(
    "Human local draft must survive",
  );
});

test("live events only invalidate native reads and never inject their raw body", async ({
  page,
}) => {
  const initial = documentState();
  const latest = documentState(
    "Live-updated runbook",
    3,
    "# Trusted refresh\n\nVerified after the signal.",
  );
  latest.revisions[DOCUMENT_ID] = [
    ...initial.revisions[DOCUMENT_ID],
    latest.revisions[DOCUMENT_ID].at(-1) as ProjectDocument,
  ];
  await installMockBridge(page, { projectDocument: initial });
  await openDocuments(page);
  await page.getByTestId(`document-list-item-${DOCUMENT_ID}`).click();
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.__BUZZ_E2E_HAS_PROJECT_DOCUMENT_SUBSCRIPTION__?.(),
      ),
    )
    .toBe(true);
  await page.evaluate((state) => {
    window.__BUZZ_E2E_SET_PROJECT_DOCUMENT_STATE__?.(state);
    window.__BUZZ_E2E_EMIT_PROJECT_DOCUMENT_EVENT__?.();
  }, latest);
  await expect(
    page.getByRole("heading", { name: "Live-updated runbook" }),
  ).toBeVisible();
  await expect(page.getByTestId("document-markdown")).toContainText(
    "Verified after the signal",
  );
  await expect(
    page.getByText("untrusted-live-body-must-not-render"),
  ).toHaveCount(0);
});

test("Community switching clears Document selection, drafts, and Relay data", async ({
  page,
}) => {
  await installMockBridge(
    page,
    {
      projectDocumentsByRelayUrl: {
        [COMMUNITY_A.relayUrl]: documentState("Alpha runbook"),
        [COMMUNITY_B.relayUrl]: documentState("Bravo handbook"),
      },
    },
    { skipCommunitySeed: true },
  );
  await seedCommunities(page);
  await openDocuments(page);
  await page.getByTestId(`document-list-item-${DOCUMENT_ID}`).click();
  await page.getByTestId("document-edit").click();
  await page.getByTestId("document-content-input").fill("Alpha-only draft");

  await page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`).click();
  await page.getByTestId("open-documents").click();
  await expect(page.getByText("Bravo handbook")).toBeVisible();
  await expect(page.getByText("Alpha runbook")).toHaveCount(0);
  await expect(page.getByTestId("document-content-input")).toHaveCount(0);
  await expect(page).not.toHaveURL(/document=/);
});

test("native integrity failures fail closed before any Document body renders", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectDocument: documentState(),
    projectDocumentReadError:
      "Project Document integrity error: signer or head pointer mismatch",
  });
  await page.goto("/");
  await page.getByTestId("open-documents").click();
  await expect(
    page.getByRole("heading", { name: "Document verification failed" }),
  ).toBeVisible();
  await expect(page.getByText("Release runbook")).toHaveCount(0);
  await expect(page.getByTestId("document-markdown")).toHaveCount(0);
});
