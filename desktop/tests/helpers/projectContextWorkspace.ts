import { expect, type Page } from "@playwright/test";

import type { AppliedWorkspaceIdentity } from "../../src/shared/api/tauri";
import type {
  ProjectContextCoordinateDetail,
  ProjectContextQueryResult,
} from "../../src/shared/api/tauriProjectContext";
import type { SemanticProjectContextQueryResult } from "../../src/shared/api/tauriProjectContextSemantic";
import type {
  ProjectDocument,
  ProjectDocumentMeta,
} from "../../src/shared/api/tauriProjectDocument";
import type {
  ProjectViewObjectType,
  RawProjectViewLoadResult,
  RawProjectViewObjectV3,
} from "../../src/shared/api/tauriProjectView";
import type { MockProjectDocumentState } from "../../src/testing/e2eBridge";

export const WORKSPACE_PROJECT_ID = "10000000-0000-4000-8000-000000000001";
export const WORKSPACE_RELAY_PUBKEY = "b".repeat(64);
export const WORKSPACE_ACTOR_PUBKEY = "a".repeat(64);
export const WORKSPACE_CONTEXT_REVISION = 42;

const UPDATED_AT = "2026-08-12T08:00:00Z";
const PROJECT_VIEW_TYPES: ProjectViewObjectType[] = [
  "requirement",
  "work",
  "resource",
  "goal",
  "role",
  "plan",
  "stage",
  "issue",
];

function fixtureUuid(prefix: string, ordinal: number): string {
  return `${prefix}-0000-4000-8000-${ordinal.toString(16).padStart(12, "0")}`;
}

function fixtureEventId(ordinal: number): string {
  return ordinal.toString(16).padStart(64, "0");
}

function coordinateDetail(ordinal: number): ProjectContextCoordinateDetail {
  const objectType = PROJECT_VIEW_TYPES[ordinal % PROJECT_VIEW_TYPES.length];
  const objectId = fixtureUuid("2a000000", ordinal + 1);
  return {
    coordinateKey: `${objectType}:${objectId}`,
    coordinate: {
      type: "project_view_object",
      objectType,
      objectId,
    },
    state: "active",
    title: `Workspace ${objectType.replace("_", " ")} ${ordinal + 1}`,
    summary: `Verified dense-canvas fixture coordinate ${ordinal + 1}.`,
    status: "active",
    objectRevision: ordinal + 1,
    updatedAt: UPDATED_AT,
    updatedBy: WORKSPACE_ACTOR_PUBKEY,
  };
}

function denseEdgeCoordinateOrdinals(): number[][] {
  const islandA: number[][] = [[0, 1, 2]];
  let previous = 2;
  let next = 3;
  for (let edge = 1; edge < 14; edge += 1) {
    islandA.push([previous, next, next + 1]);
    previous = next + 1;
    next += 2;
  }
  islandA.push([previous, 29]);

  const islandB: number[][] = [[30, 31, 32, 33]];
  previous = 33;
  next = 34;
  for (let edge = 1; edge < 6; edge += 1) {
    islandB.push([previous, next, next + 1]);
    previous = next + 1;
    next += 2;
  }
  return [...islandA, ...islandB];
}

export type DenseProjectContextFixture = {
  result: ProjectContextQueryResult;
  coordinateKeys: string[];
  edgeKeys: string[];
  documentIds: string[];
};

/** The dense two-island fixture required by the full-canvas visual contract. */
export function denseProjectContextFixture(): DenseProjectContextFixture {
  const coordinateDetails = Array.from({ length: 44 }, (_, ordinal) =>
    coordinateDetail(ordinal),
  );
  const coordinateKeys = coordinateDetails.map(
    (coordinate) => coordinate.coordinateKey,
  );
  const edgeKeys = Array.from({ length: 21 }, (_, ordinal) =>
    fixtureEventId(ordinal + 1),
  );
  const documentIds = Array.from({ length: 22 }, (_, ordinal) =>
    fixtureUuid("4a000000", ordinal + 1),
  );
  const edges = denseEdgeCoordinateOrdinals().map((ordinals, edgeIndex) => ({
    edgeKey: edgeKeys[edgeIndex],
    coordinateKeys: ordinals.map((ordinal) => coordinateKeys[ordinal]),
    contextDocumentIds:
      edgeIndex === 0
        ? [documentIds[edgeIndex], documentIds[21]]
        : [documentIds[edgeIndex]],
  }));
  return {
    coordinateKeys,
    documentIds,
    edgeKeys,
    result: {
      communityKey: "workspace-fixture",
      projectId: WORKSPACE_PROJECT_ID,
      relayPubkey: WORKSPACE_RELAY_PUBKEY,
      context: {
        contextRevision: WORKSPACE_CONTEXT_REVISION,
        projectionGeneration: 3,
        activeEdgeCount: edges.length,
        boundDocumentCount: documentIds.length,
        updatedAt: UPDATED_AT,
        metaEventId: "c".repeat(64),
        capabilityEnabled: true,
        semanticQueryAvailable: true,
      },
      query: { type: "contains_all", coordinates: [] },
      projectViewObservation: {
        state: "observed",
        projectRevision: 18,
        projectionGeneration: 5,
        updatedAt: UPDATED_AT,
        metaEventId: "d".repeat(64),
      },
      documentObservation: {
        state: "observed",
        catalogRevision: 12,
        projectionGeneration: 4,
        updatedAt: UPDATED_AT,
        metaEventId: "e".repeat(64),
      },
      meetingObservations: [],
      edges,
      coordinateDetails,
      documentDetails: documentIds.map((documentId, ordinal) => ({
        documentId,
        state: "active",
        title: `Context rationale ${ordinal + 1}`,
        summary: `Verified relation document ${ordinal + 1}.`,
        documentRevision: ordinal + 1,
        updatedAt: UPDATED_AT,
        updatedBy: WORKSPACE_ACTOR_PUBKEY,
      })),
    },
  };
}

const workspaceProfile = {
  id: WORKSPACE_PROJECT_ID,
  object_type: "project_profile",
  object_revision: 3,
  project_revision: 18,
  created_at: UPDATED_AT,
  updated_at: UPDATED_AT,
  created_by: WORKSPACE_ACTOR_PUBKEY,
  updated_by: WORKSPACE_ACTOR_PUBKEY,
  data: {
    object_type: "project_profile",
    data: {
      name: "Dense Project Context workspace",
      positioning: "A full-canvas Project Context fixture.",
      purpose: "Exercise workspace presentation without changing graph truth.",
      problem: "Dense Context needs a readable canvas.",
      scope: "Two islands, 44 coordinates, 21 edges, and 22 documents.",
    },
  },
  relations: {},
  context_references: [],
} satisfies RawProjectViewObjectV3;

const workspaceGoal = {
  id: "3a000000-0000-4000-8000-000000000001",
  object_type: "goal",
  object_revision: 1,
  project_revision: 18,
  created_at: UPDATED_AT,
  updated_at: UPDATED_AT,
  created_by: WORKSPACE_ACTOR_PUBKEY,
  updated_by: WORKSPACE_ACTOR_PUBKEY,
  data: {
    object_type: "goal",
    data: {
      title: "Keep dense Context inspectable",
      desired_outcome: "Every verified Coordinate remains understandable.",
      directions: ["Preserve canonical detail while navigating the graph"],
    },
  },
  relations: {},
  context_references: [],
} satisfies RawProjectViewObjectV3;

/** Canonical Project View subset used when the first Coordinate opens Details. */
export function denseProjectViewFixture(
  dense = denseProjectContextFixture(),
): RawProjectViewLoadResult {
  const first = dense.result.coordinateDetails[0];
  if (first.coordinate.type !== "project_view_object") {
    throw new Error("Dense fixture must start with a Project View Coordinate.");
  }
  const selectedRequirement = {
    id: first.coordinate.objectId,
    object_type: "requirement",
    object_revision: 1,
    project_revision: 18,
    created_at: UPDATED_AT,
    updated_at: UPDATED_AT,
    created_by: WORKSPACE_ACTOR_PUBKEY,
    updated_by: WORKSPACE_ACTOR_PUBKEY,
    data: {
      object_type: "requirement",
      data: {
        title: "Workspace requirement 1",
        description: "Keep the dense Project Context graph readable.",
        summary: "The selected canonical requirement.",
        status: "ready",
        priority: "high",
      },
    },
    relations: {},
    context_references: [],
  } satisfies RawProjectViewObjectV3;
  return {
    status: "ready",
    relay_pubkey: WORKSPACE_RELAY_PUBKEY,
    project_context_supported: true,
    schema_version: 3,
    project_revision: 18,
    projection_generation: 5,
    active_object_count: 3,
    updated_at: UPDATED_AT,
    objects_v3: [workspaceProfile, workspaceGoal, selectedRequirement],
    role_continuity: {
      roles: [],
      proposals: [],
      assignments: [],
      commitments: [],
      workResponsibilities: [],
      checkpoints: [],
      handoffs: [],
      members: [],
      briefs: [],
    },
  };
}

/** Canonical Document catalog used by Edge Details in the workspace spec. */
export function denseProjectDocumentFixture(
  dense = denseProjectContextFixture(),
): MockProjectDocumentState {
  const documents: ProjectDocument[] = dense.documentIds.map(
    (documentId, ordinal) => ({
      communityKey: "workspace-fixture",
      projectId: WORKSPACE_PROJECT_ID,
      relayPubkey: WORKSPACE_RELAY_PUBKEY,
      projectionGeneration: 4,
      documentId,
      documentRevision: ordinal + 1,
      state: "active",
      title: `Context rationale ${ordinal + 1}`,
      summary: `Verified relation document ${ordinal + 1}.`,
      contentMarkdown: `# Context rationale ${ordinal + 1}`,
      createdAt: UPDATED_AT,
      createdBy: WORKSPACE_ACTOR_PUBKEY,
      revisionAt: UPDATED_AT,
      revisionBy: WORKSPACE_ACTOR_PUBKEY,
      revisionEventId: fixtureEventId(100 + ordinal),
      headEventId: fixtureEventId(100 + ordinal),
      sourceEventId: fixtureEventId(100 + ordinal),
    }),
  );
  const meta: ProjectDocumentMeta = {
    communityKey: "workspace-fixture",
    projectId: WORKSPACE_PROJECT_ID,
    relayPubkey: WORKSPACE_RELAY_PUBKEY,
    projectionGeneration: 4,
    catalogRevision: 12,
    activeDocumentCount: documents.length,
    updatedAt: UPDATED_AT,
    metaEventId: "f".repeat(64),
  };
  return {
    meta,
    documents: documents.map((document) => ({
      documentId: document.documentId,
      title: document.title ?? "Context rationale",
      summary: document.summary,
      documentRevision: document.documentRevision,
      updatedAt: document.revisionAt,
      updatedBy: document.revisionBy,
      headEventId: document.headEventId ?? document.revisionEventId,
    })),
    revisions: Object.fromEntries(
      documents.map((document) => [document.documentId, [document]]),
    ),
  };
}

export function semanticWorkspaceResult(
  workspace: AppliedWorkspaceIdentity,
  dense = denseProjectContextFixture(),
  coverage: {
    budgetExhausted?: boolean;
    partialCoverage?: boolean;
  } = {},
): SemanticProjectContextQueryResult {
  const rootCoordinateKey = dense.coordinateKeys[0];
  const terminalCoordinateKey = dense.coordinateKeys[2];
  return {
    communityKey: workspace.communityKey,
    appliedWorkspaceToken: workspace.appliedWorkspaceToken,
    callerPubkey: workspace.callerPubkey,
    requestId: "6a000000-0000-4000-8000-000000000001",
    projectId: WORKSPACE_PROJECT_ID,
    relayPubkey: WORKSPACE_RELAY_PUBKEY,
    projectContextRevision: WORKSPACE_CONTEXT_REVISION,
    snapshotObservedAt: UPDATED_AT,
    completionReason: coverage.budgetExhausted
      ? "budget_exhausted"
      : "frontier_exhausted",
    exhaustedDimensions: coverage.budgetExhausted ? ["paths"] : [],
    coverage: {
      authorizedGraphSources: 66,
      currentIndexedGraphSources: coverage.partialCoverage ? 65 : 66,
      titleOnlySources: 0,
      rootsReturned: 1,
      pathsReturned: 1,
      omittedInitialCoordinates: 0,
      omittedContextCoordinates: 0,
      indexCoveragePartial: coverage.partialCoverage ? 1 : 0,
      omittedForResponseBudget: {
        automaticRoots: 0,
        paths: 0,
        summaries: 0,
      },
    },
    inputOutcomes: { initial: [], context: [] },
    roots: [
      {
        rootId: "workspace-semantic-root",
        coordinateEntrypoints: [rootCoordinateKey],
        contextDocumentEntrypoints: [],
      },
    ],
    paths: [
      {
        pathId: "workspace-semantic-path",
        rootId: "workspace-semantic-root",
        branchStopReason: "frontier_exhausted",
        hops: [
          {
            ordinal: 0,
            edgeKey: dense.edgeKeys[0],
            completeCoordinateKeys: dense.result.edges[0].coordinateKeys,
            currentContextDocumentIds: dense.result.edges[0].contextDocumentIds,
            enteredFromCoordinateKey: rootCoordinateKey,
            selectedContextDocumentId: dense.documentIds[0],
            continuedToCoordinateKey: terminalCoordinateKey,
          },
        ],
      },
    ],
  };
}

export async function openProjectContextWorkspace(page: Page): Promise<void> {
  await page.goto("/");
  await page.getByTestId("open-project-context").click();
  await expect(page).toHaveURL(/#\/project-context$/);
  await expect(page.getByTestId("project-context-graph")).toBeVisible();
}

export async function installWorkspaceSemanticResult(
  page: Page,
  dense: DenseProjectContextFixture,
  coverage: {
    budgetExhausted?: boolean;
    partialCoverage?: boolean;
  } = {},
): Promise<SemanticProjectContextQueryResult> {
  await expect
    .poll(() =>
      page.evaluate(() => window.__BUZZ_E2E_APPLIED_WORKSPACE__ ?? null),
    )
    .not.toBeNull();
  const workspace = await page.evaluate(
    () => window.__BUZZ_E2E_APPLIED_WORKSPACE__ ?? null,
  );
  if (!workspace) {
    throw new Error("Mock workspace identity was not applied.");
  }
  const semantic = semanticWorkspaceResult(workspace, dense, coverage);
  const installed = await page.evaluate((result) => {
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT_SEMANTIC__?.(result);
    return Boolean(window.__BUZZ_E2E_SET_PROJECT_CONTEXT_SEMANTIC__);
  }, semantic);
  expect(installed).toBe(true);
  return semantic;
}
