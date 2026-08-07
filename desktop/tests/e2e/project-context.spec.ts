import { expect, test } from "@playwright/test";

import type {
  ProjectContextErrorPayload,
  ProjectContextQuery,
  ProjectContextQueryResult,
} from "../../src/shared/api/tauriProjectContext";
import type { MeetingSnapshot } from "../../src/shared/api/tauriMeetings";
import type {
  ProjectDocument,
  ProjectDocumentMeta,
} from "../../src/shared/api/tauriProjectDocument";
import type {
  RawProjectViewLoadResult,
  RawProjectViewObjectV3,
} from "../../src/shared/api/tauriProjectView";
import {
  KIND_PROJECT_CONTEXT_EDGE_BINDING,
  KIND_PROJECT_DOCUMENT_HEAD,
  KIND_PROJECT_VIEW_OBJECT,
} from "../../src/shared/constants/kinds";
import type { MockProjectDocumentState } from "../../src/testing/e2eBridge";
import { waitForAnimations } from "../helpers/animations";
import { installMockBridge, TEST_IDENTITIES } from "../helpers/bridge";

const PROJECT_ID = "10000000-0000-4000-8000-000000000001";
const REQUIREMENT_ID = "20000000-0000-4000-8000-000000000001";
const RESOURCE_ID = "30000000-0000-4000-8000-000000000001";
const GOAL_ID = "30000000-0000-4000-8000-000000000002";
const ROLE_ID = "30000000-0000-4000-8000-000000000003";
const VIEW_GOAL_ID = "30000000-0000-4000-8000-000000000004";
const DOCUMENT_COORDINATE_ID = "40000000-0000-4000-8000-000000000001";
const CONTEXT_DOCUMENT_A_ID = "40000000-0000-4000-8000-000000000002";
const CONTEXT_DOCUMENT_B_ID = "40000000-0000-4000-8000-000000000003";
const CONTEXT_DOCUMENT_C_ID = "40000000-0000-4000-8000-000000000004";
const CONTEXT_DOCUMENT_D_ID = "40000000-0000-4000-8000-000000000005";
const MEETING_ID = "50000000-0000-4000-8000-000000000001";
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
    meetingObservations: [],
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

function terminalMeetingSnapshot(): MeetingSnapshot {
  return {
    meetingId: MEETING_ID,
    title: "Memory boundary review",
    description: "Agree the first durable Agent memory slice.",
    sourceChannelId: null,
    schemaVersion: 3,
    policy: "moderated-board-actions-v3",
    hostPubkey: TEST_IDENTITIES.alice.pubkey,
    moderatorPubkey: TEST_IDENTITIES.alice.pubkey,
    createEventId: "5".repeat(64),
    createdAt: 1_786_054_800,
    lifecycle: "closed",
    phase: "ended",
    stateRevision: 64,
    floorRevision: 12,
    intentRevision: 4,
    speechRevision: 6,
    currentSpeakerPubkey: null,
    currentOfferPubkey: null,
    floor: null,
    host: null,
    participants: [
      {
        pubkey: TEST_IDENTITIES.alice.pubkey,
        participantType: "human",
        channelRole: "admin",
      },
      {
        pubkey: TEST_IDENTITIES.charlie.pubkey,
        participantType: "agent",
        channelRole: "member",
      },
    ],
    board: {
      eventId: "6".repeat(64),
      format: "markdown",
      body: "# Private to the Meeting route\nThis must not render in Context.",
      moderatorPubkey: TEST_IDENTITIES.alice.pubkey,
      updatedAt: 1_786_055_350,
      source: "projection",
    },
    action: {
      actionRunId: "51000000-0000-4000-8000-000000000001",
      boardEventId: "6".repeat(64),
      actionWindowEpoch: 2,
      condition: "recorded",
      terminalStatus: "completed_closed",
      completionEventId: "7".repeat(64),
      actionDeadlineAtMs: null,
      progressSeq: 3,
      lastProgressStage: "recorded",
      lastProgressAtMs: 1_786_055_380_000,
      operatorHardDeadlineMs: null,
      createdAtMs: 1_786_055_300_000,
      lastErrorCode: null,
    },
    end: {
      eventId: "8".repeat(64),
      outcome: "closed",
      reasonCode: null,
      reason: null,
      endedBy: TEST_IDENTITIES.alice.pubkey,
      endedAt: 1_786_055_400,
      actionsAttested: true,
      terminationSource: "host",
    },
    latestSpeechAt: 1_786_055_300,
  };
}

function meetingContextResult(): ProjectContextQueryResult {
  const base = contextResult();
  return {
    ...base,
    meetingObservations: [
      {
        meetingId: MEETING_ID,
        state: "observed",
        stateRevision: 64,
        createEventId: "5".repeat(64),
        stateEventId: "9".repeat(64),
        endEventId: "8".repeat(64),
        updatedAt: "2026-08-07T22:30:00Z",
      },
    ],
    edges: [
      {
        edgeKey: "3".repeat(64),
        coordinateKeys: [
          `requirement:${REQUIREMENT_ID}`,
          `meeting:${MEETING_ID}`,
        ],
        contextDocumentIds: [CONTEXT_DOCUMENT_A_ID],
      },
    ],
    coordinateDetails: [
      {
        coordinateKey: `requirement:${REQUIREMENT_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId: REQUIREMENT_ID,
        },
        state: "active",
        title: "Durable memory boundary",
      },
      {
        coordinateKey: `meeting:${MEETING_ID}`,
        coordinate: { type: "meeting", meetingId: MEETING_ID },
        state: "terminal",
        title: "Memory boundary review",
        status: "closed",
        meeting: {
          discussionGoal: "Agree the first durable Agent memory slice.",
          terminalOutcome: "closed",
          hostPubkey: TEST_IDENTITIES.alice.pubkey,
          participantCount: 2,
          participantPreview: [
            {
              pubkey: TEST_IDENTITIES.alice.pubkey,
              participantType: "human",
            },
            {
              pubkey: TEST_IDENTITIES.charlie.pubkey,
              participantType: "agent",
            },
          ],
          createdAt: "2026-08-07T22:20:00Z",
          endedAt: "2026-08-07T22:30:00Z",
          actionFinalization: {
            condition: "recorded",
            terminalStatus: "completed_closed",
            actionsAttested: true,
          },
        },
      },
    ],
  };
}

const inspectorProfile = {
  id: PROJECT_ID,
  object_type: "project_profile",
  object_revision: 3,
  project_revision: 11,
  created_at: "2026-08-01T08:00:00Z",
  updated_at: "2026-08-06T08:00:00Z",
  created_by: ACTOR,
  updated_by: ACTOR,
  data: {
    object_type: "project_profile",
    data: {
      name: "Trusted Context workspace",
      positioning: "A verified map of cross-coordinate explanations.",
      purpose: "Keep project relationships understandable.",
      problem: "Relevant reasoning otherwise remains fragmented.",
      scope: "Project View objects and Documents.",
    },
  },
  relations: {},
  context_references: [],
} satisfies RawProjectViewObjectV3;

const inspectorRequirement = {
  id: REQUIREMENT_ID,
  object_type: "requirement",
  object_revision: 4,
  project_revision: 11,
  created_at: "2026-08-01T08:00:00Z",
  updated_at: "2026-08-06T08:00:00Z",
  created_by: ACTOR,
  updated_by: ACTOR,
  data: {
    object_type: "requirement",
    data: {
      title: "Keep Context relationships verifiable",
      description:
        "Every visible relationship must come from one complete verified Edge.",
      status: "ready",
      priority: "high",
    },
  },
  relations: {
    about: { object_type: "resource", object_id: RESOURCE_ID },
  },
  context_references: [],
} satisfies RawProjectViewObjectV3;

const inspectorGoal = {
  id: VIEW_GOAL_ID,
  object_type: "goal",
  object_revision: 1,
  project_revision: 11,
  created_at: "2026-08-01T08:00:00Z",
  updated_at: "2026-08-06T08:00:00Z",
  created_by: ACTOR,
  updated_by: ACTOR,
  data: {
    object_type: "goal",
    data: {
      title: "Make project reasoning inspectable",
      desired_outcome: "Humans and Agents can inspect verified Context.",
      directions: ["Keep the graph body-free until selection"],
    },
  },
  relations: {},
  context_references: [],
} satisfies RawProjectViewObjectV3;

const inspectorResource = {
  id: RESOURCE_ID,
  object_type: "resource",
  object_revision: 2,
  project_revision: 11,
  created_at: "2026-08-01T08:00:00Z",
  updated_at: "2026-08-06T08:00:00Z",
  created_by: ACTOR,
  updated_by: ACTOR,
  data: {
    object_type: "resource",
    data: {
      name: "Project Context contract",
      resource_kind: "document",
      summary: "The reviewed Project Context domain contract.",
      guide_document_id: CONTEXT_DOCUMENT_A_ID,
    },
  },
  relations: {},
  context_references: [],
} satisfies RawProjectViewObjectV3;

function inspectorProjectView(): RawProjectViewLoadResult {
  return {
    status: "ready",
    relay_pubkey: RELAY,
    project_context_supported: true,
    schema_version: 3,
    project_revision: 11,
    projection_generation: 3,
    active_object_count: 4,
    updated_at: "2026-08-06T08:00:00Z",
    objects_v3: [
      inspectorProfile,
      inspectorGoal,
      inspectorRequirement,
      inspectorResource,
    ],
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

function contextDocumentSnapshot(input: {
  contentMarkdown: string;
  documentId: string;
  eventDigit: string;
  revision: number;
  summary: string;
  title: string;
}): ProjectDocument {
  return {
    communityKey: "fixture",
    projectId: PROJECT_ID,
    relayPubkey: RELAY,
    projectionGeneration: 2,
    documentId: input.documentId,
    documentRevision: input.revision,
    state: "active",
    title: input.title,
    summary: input.summary,
    contentMarkdown: input.contentMarkdown,
    createdAt: "2026-08-01T08:00:00Z",
    createdBy: ACTOR,
    revisionAt: "2026-08-06T08:00:00Z",
    revisionBy: ACTOR,
    revisionEventId: input.eventDigit.repeat(64),
    headEventId: input.eventDigit.repeat(64),
    sourceEventId: input.eventDigit.repeat(64),
  };
}

function inspectorDocumentState(): MockProjectDocumentState {
  const documents = [
    contextDocumentSnapshot({
      contentMarkdown:
        "# Architecture rationale A\n\nOnly the first Context body is rendered.",
      documentId: CONTEXT_DOCUMENT_A_ID,
      eventDigit: "1",
      revision: 8,
      summary: "Current summary for the architecture rationale.",
      title: "Current architecture rationale",
    }),
    contextDocumentSnapshot({
      contentMarkdown:
        "# Operational rationale B\n\nThis body is fetched only after switching Documents.",
      documentId: CONTEXT_DOCUMENT_B_ID,
      eventDigit: "2",
      revision: 2,
      summary: "Current summary for the operational rationale.",
      title: "Current operational rationale",
    }),
    contextDocumentSnapshot({
      contentMarkdown:
        "# Ownership rationale C\n\nThe second Edge owns this Context Document.",
      documentId: CONTEXT_DOCUMENT_C_ID,
      eventDigit: "3",
      revision: 1,
      summary: "Current summary for the ownership rationale.",
      title: "Current ownership rationale",
    }),
  ];
  const meta: ProjectDocumentMeta = {
    communityKey: "fixture",
    projectId: PROJECT_ID,
    relayPubkey: RELAY,
    projectionGeneration: 2,
    catalogRevision: 8,
    activeDocumentCount: documents.length,
    updatedAt: "2026-08-06T08:00:00Z",
    metaEventId: "4".repeat(64),
  };
  return {
    meta,
    documents: documents.map((document) => ({
      documentId: document.documentId,
      title: document.title ?? "Context Document",
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

function inspectorResult(): ProjectContextQueryResult {
  const base = contextResult({ edgeCount: 2 });
  return {
    ...base,
    context: {
      ...base.context,
      activeEdgeCount: 2,
      boundDocumentCount: 3,
    },
    edges: [
      {
        edgeKey: "1".repeat(64),
        coordinateKeys: [
          `goal:${GOAL_ID}`,
          `requirement:${REQUIREMENT_ID}`,
          `resource:${RESOURCE_ID}`,
        ],
        contextDocumentIds: [CONTEXT_DOCUMENT_A_ID, CONTEXT_DOCUMENT_B_ID],
      },
      {
        edgeKey: "2".repeat(64),
        coordinateKeys: [
          `document:${CONTEXT_DOCUMENT_A_ID}`,
          `requirement:${REQUIREMENT_ID}`,
          `role:${ROLE_ID}`,
        ],
        contextDocumentIds: [CONTEXT_DOCUMENT_C_ID],
      },
    ],
    coordinateDetails: [
      {
        coordinateKey: `goal:${GOAL_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "goal",
          objectId: GOAL_ID,
        },
        state: "tombstoned",
        title: "Retired Context milestone",
        objectRevision: 6,
        updatedAt: "2026-08-05T08:00:00Z",
        updatedBy: ACTOR,
      },
      {
        coordinateKey: `requirement:${REQUIREMENT_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId: REQUIREMENT_ID,
        },
        state: "active",
        title: "Verified requirement Coordinate",
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
        title: "Verified resource Coordinate",
        objectRevision: 2,
        updatedAt: "2026-08-06T08:00:00Z",
        updatedBy: ACTOR,
      },
      {
        coordinateKey: `document:${CONTEXT_DOCUMENT_A_ID}`,
        coordinate: {
          type: "document",
          documentId: CONTEXT_DOCUMENT_A_ID,
        },
        state: "active",
        title: "Architecture record Coordinate",
        documentRevision: 8,
        updatedAt: "2026-08-06T08:00:00Z",
        updatedBy: ACTOR,
      },
      {
        coordinateKey: `role:${ROLE_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "role",
          objectId: ROLE_ID,
        },
        state: "unavailable",
        unavailableReason: "Role details are temporarily unavailable.",
      },
    ],
    documentDetails: [
      {
        documentId: CONTEXT_DOCUMENT_A_ID,
        state: "active",
        title: "Context binding A",
        summary: "Observed summary for the first binding.",
        documentRevision: 8,
        updatedAt: "2026-08-06T08:00:00Z",
        updatedBy: ACTOR,
      },
      {
        documentId: CONTEXT_DOCUMENT_B_ID,
        state: "active",
        title: "Context binding B",
        summary: "Observed summary for the second binding.",
        documentRevision: 2,
        updatedAt: "2026-08-06T08:00:00Z",
        updatedBy: ACTOR,
      },
      {
        documentId: CONTEXT_DOCUMENT_C_ID,
        state: "active",
        title: "Context binding C",
        summary: "Observed summary for the independent Edge binding.",
        documentRevision: 1,
        updatedAt: "2026-08-06T08:00:00Z",
        updatedBy: ACTOR,
      },
    ],
  };
}

function twoIslandResult(): ProjectContextQueryResult {
  const base = contextResult();
  return {
    ...base,
    context: {
      ...base.context,
      activeEdgeCount: 3,
      boundDocumentCount: 3,
    },
    edges: [
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
          `goal:${GOAL_ID}`,
          `requirement:${REQUIREMENT_ID}`,
          `resource:${RESOURCE_ID}`,
        ],
        contextDocumentIds: [CONTEXT_DOCUMENT_B_ID],
      },
      {
        edgeKey: "3".repeat(64),
        coordinateKeys: [
          `role:${ROLE_ID}`,
          `document:${CONTEXT_DOCUMENT_A_ID}`,
        ],
        contextDocumentIds: [CONTEXT_DOCUMENT_C_ID],
      },
    ],
    coordinateDetails: [
      {
        coordinateKey: `goal:${GOAL_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "goal",
          objectId: GOAL_ID,
        },
        state: "active",
        title: "Ship the trusted project workspace",
        objectRevision: 5,
        updatedAt: "2026-08-06T08:00:00Z",
        updatedBy: ACTOR,
      },
      {
        coordinateKey: `requirement:${REQUIREMENT_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId: REQUIREMENT_ID,
        },
        state: "active",
        title: "Keep Context relationships verifiable",
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
        state: "tombstoned",
        title: "Legacy relay contract",
        objectRevision: 7,
        updatedAt: "2026-08-06T08:00:00Z",
        updatedBy: ACTOR,
      },
      {
        coordinateKey: `role:${ROLE_ID}`,
        coordinate: {
          type: "project_view_object",
          objectType: "role",
          objectId: ROLE_ID,
        },
        state: "unavailable",
        unavailableReason: "Role details are temporarily unavailable.",
      },
      {
        coordinateKey: `document:${CONTEXT_DOCUMENT_A_ID}`,
        coordinate: {
          type: "document",
          documentId: CONTEXT_DOCUMENT_A_ID,
        },
        state: "active",
        title: "Cross-team architecture record",
        documentRevision: 8,
        updatedAt: "2026-08-06T08:00:00Z",
        updatedBy: ACTOR,
      },
    ],
    documentDetails: [
      {
        documentId: CONTEXT_DOCUMENT_A_ID,
        state: "active",
        title: "Context rationale A",
        documentRevision: 8,
      },
      {
        documentId: CONTEXT_DOCUMENT_B_ID,
        state: "active",
        title: "Context rationale B",
        documentRevision: 2,
      },
      {
        documentId: CONTEXT_DOCUMENT_C_ID,
        state: "active",
        title: "Context rationale C",
        documentRevision: 1,
      },
    ],
  };
}

function queryKey(query: ProjectContextQuery): string {
  return JSON.stringify(query);
}

function focusedResult(
  query: ProjectContextQuery,
  input?: { noMatch?: boolean },
): ProjectContextQueryResult {
  const base = contextResult();
  if (!input?.noMatch) return { ...base, query };
  const coordinates =
    query.type === "incident" ? [query.coordinate] : query.coordinates;
  return {
    ...base,
    query,
    edges: [],
    coordinateDetails: coordinates.map((coordinate) => ({
      coordinateKey:
        coordinate.type === "document"
          ? `document:${coordinate.documentId}`
          : `${coordinate.objectType}:${coordinate.objectId}`,
      coordinate,
      state: "active",
      title: "Unmatched query anchor",
    })),
    documentDetails: [],
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

async function documentBodyCalls(page: import("@playwright/test").Page) {
  return page.evaluate(() =>
    window.__BUZZ_E2E_PROJECT_DOCUMENT_CALLS__?.filter(
      (call) => call.command === "get_project_document",
    ),
  );
}

async function waitForProjectContextLive(
  page: import("@playwright/test").Page,
) {
  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_HAS_PROJECT_CONTEXT_SUBSCRIPTION__?.() ?? false,
      ),
    )
    .toBe(true);
  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
      ),
    )
    .toBeGreaterThanOrEqual(2);
  await expect(page.getByTestId("project-context-sync-status")).toHaveText(
    "Live",
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
  expect(calls?.length).toBeGreaterThanOrEqual(1);
  expect(
    calls?.every((call) =>
      JSON.stringify(call.payload).includes('"type":"contains_all"'),
    ),
  ).toBe(true);
});

test("Project View Coordinate Inspector is read-only, responsive, and restores graph focus", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockBridge(page, {
    projectContext: inspectorResult(),
    projectDocument: inspectorDocumentState(),
    projectView: inspectorProjectView(),
  });
  await openProjectContext(page);

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
      ),
    )
    .toBeGreaterThanOrEqual(2);
  const callsBeforeInspectorInteraction = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
  );

  expect(await documentBodyCalls(page)).toHaveLength(0);
  const requirementNode = page.getByTestId(
    `project-context-coordinate-requirement:${REQUIREMENT_ID}`,
  );
  await requirementNode.click();

  const inspector = page.getByTestId("project-context-inspector");
  await expect(inspector).toBeVisible();
  await expect(
    inspector.getByTestId("project-context-project-view-detail"),
  ).toContainText("Keep Context relationships verifiable");
  await expect(inspector).toContainText(
    "Every visible relationship must come from one complete verified Edge.",
  );
  await expect(inspector).toContainText("Ready");
  await expect(inspector).toContainText("High");
  await expect(inspector).toContainText("Project Context contract");
  await expect(inspector.getByRole("button", { name: /edit/i })).toHaveCount(0);
  await expect(inspector.getByRole("button", { name: /delete/i })).toHaveCount(
    0,
  );
  await expect(page).toHaveURL(/selected=coordinate/);
  await expect(page).not.toHaveURL(/mode=/);
  expect(await documentBodyCalls(page)).toHaveLength(0);

  expect(
    await inspector.evaluate((element) => getComputedStyle(element).position),
  ).toBe("relative");
  await page.setViewportSize({ width: 560, height: 800 });
  await expect
    .poll(() =>
      inspector.evaluate((element) => getComputedStyle(element).position),
    )
    .toBe("fixed");
  const narrowBox = await inspector.boundingBox();
  expect(narrowBox?.width).toBeGreaterThan(400);
  await waitForAnimations(page);
  await page.screenshot({
    path: "test-results/project-context/project-context-narrow-sheet.png",
  });
  expect(
    await page.evaluate(
      () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length,
    ),
  ).toBe(callsBeforeInspectorInteraction);

  await page.setViewportSize({ width: 1280, height: 800 });
  await inspector.getByTestId("project-context-open-project-view").click();
  await expect(page).toHaveURL(new RegExp(`/view\\?object=${REQUIREMENT_ID}$`));
  await page.goBack();
  await expect(page.getByTestId("project-context-inspector")).toBeVisible();
  await expect(page).toHaveURL(/selected=coordinate/);

  await page.getByTestId("project-context-show-incident").click();
  await expect(page).toHaveURL(/mode=incident/);
  await expect(page).toHaveURL(
    new RegExp(`coordinates=requirement(%3A|:)${REQUIREMENT_ID}`),
  );
  await expect(page).not.toHaveURL(/selected=/);
  await requirementNode.click();
  await page.getByTestId("project-context-focus-selection").click();
  await expect(requirementNode.locator("button")).toBeFocused();

  await page.keyboard.press("Escape");
  await expect(page.getByTestId("project-context-inspector")).toHaveCount(0);
  await expect(page).not.toHaveURL(/selected=/);
  await expect(requirementNode.locator("button")).toBeFocused();
  expect(await documentBodyCalls(page)).toHaveLength(0);
});

test("Meeting Coordinate stays metadata-first and opens the Community-readable Meeting route", async ({
  page,
}) => {
  await installMockBridge(page, {
    meetings: [
      {
        id: MEETING_ID,
        title: "Memory boundary review",
        result: { status: "ready", snapshot: terminalMeetingSnapshot() },
        speeches: [],
        activities: [],
      },
    ],
    projectContext: meetingContextResult(),
  });
  await openProjectContext(page);

  await page
    .getByTestId(`project-context-coordinate-meeting:${MEETING_ID}`)
    .click();
  const inspector = page.getByTestId("project-context-inspector");
  const meeting = inspector.getByTestId("project-context-meeting-detail");
  await expect(meeting).toBeVisible();
  await expect(meeting).toContainText("Memory boundary review");
  await expect(meeting).toContainText(
    "Agree the first durable Agent memory slice.",
  );
  await expect(meeting).toContainText("completed_closed");
  await expect(meeting).not.toContainText("Private to the Meeting route");
  await expect(page).toHaveURL(/selected=coordinate/);

  await meeting.getByTestId("project-context-open-meeting").click();
  await expect(page).toHaveURL(new RegExp(`/channels/${MEETING_ID}$`));
  await page.getByTestId("meeting-history-trigger").click();
  await expect(
    page.getByTestId(`meeting-history-row-${MEETING_ID}`),
  ).toContainText("Closed · Observer");
  await expect(page.getByTestId("meeting-speech-composer")).toHaveCount(0);
  await expect(page.getByTestId("meeting-host-console")).toHaveCount(0);
  await expect(page.getByTestId("meeting-agent-activity-row")).toHaveCount(0);

  await page.goBack();
  await expect(page.getByTestId("project-context-inspector")).toBeVisible();
  await expect(page).toHaveURL(/selected=coordinate/);
});

test("Document Coordinate lazily reads current Markdown and returns from Documents", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectContext: inspectorResult(),
    projectDocument: inspectorDocumentState(),
    projectView: inspectorProjectView(),
  });
  await openProjectContext(page);
  expect(await documentBodyCalls(page)).toHaveLength(0);

  await page
    .getByTestId(`project-context-coordinate-document:${CONTEXT_DOCUMENT_A_ID}`)
    .click();
  const inspector = page.getByTestId("project-context-inspector");
  await expect(inspector).toBeVisible();
  await expect(inspector).toContainText("Current architecture rationale");
  await expect(
    inspector.getByTestId(
      `project-context-document-body-${CONTEXT_DOCUMENT_A_ID}`,
    ),
  ).toContainText("Only the first Context body is rendered.");
  await expect(inspector).toContainText("Revision 8");
  await expect(inspector.getByRole("button", { name: /edit/i })).toHaveCount(0);
  await expect(inspector.getByRole("button", { name: /delete/i })).toHaveCount(
    0,
  );
  let bodyCalls = await documentBodyCalls(page);
  expect(bodyCalls).toHaveLength(1);
  expect(bodyCalls?.[0]?.payload).toMatchObject({
    input: { documentId: CONTEXT_DOCUMENT_A_ID },
  });

  await inspector
    .getByTestId(`project-context-open-document-${CONTEXT_DOCUMENT_A_ID}`)
    .click();
  await expect(page).toHaveURL(
    new RegExp(`/documents\\?document=${CONTEXT_DOCUMENT_A_ID}$`),
  );
  await expect(page.getByTestId("document-viewer")).toBeVisible();
  await page.goBack();
  await expect(page.getByTestId("project-context-inspector")).toBeVisible();
  await expect(page).toHaveURL(/selected=coordinate/);
  bodyCalls = await documentBodyCalls(page);
  expect(bodyCalls).toHaveLength(1);
});

test("a Spoke opens the complete Edge and multi-Document bodies stay independent", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectContext: inspectorResult(),
    projectDocument: inspectorDocumentState(),
    projectView: inspectorProjectView(),
  });
  await openProjectContext(page);
  expect(await documentBodyCalls(page)).toHaveLength(0);

  const firstEdgeKey = "1".repeat(64);
  const firstSpokeId = `spoke:${firstEdgeKey}:goal:${GOAL_ID}`;
  const firstSpoke = page.locator(
    `.react-flow__edge[data-id="${firstSpokeId}"] .react-flow__edge-interaction`,
  );
  await expect(firstSpoke).toHaveCount(1);
  await firstSpoke.dispatchEvent("click");

  const inspector = page.getByTestId("project-context-inspector");
  const edgeInspector = inspector.getByTestId("project-context-edge-inspector");
  await expect(edgeInspector).toBeVisible();
  await expect(inspector.locator("h2")).toHaveText("Context Edge");
  await expect(
    edgeInspector.locator('[data-testid^="project-context-edge-coordinate-"]'),
  ).toHaveCount(3);
  await expect(
    edgeInspector.getByTestId(
      `project-context-edge-document-${CONTEXT_DOCUMENT_A_ID}`,
    ),
  ).toBeVisible();
  await expect(
    edgeInspector.getByTestId(
      `project-context-edge-document-${CONTEXT_DOCUMENT_B_ID}`,
    ),
  ).toBeVisible();
  await expect(
    edgeInspector.getByTestId("project-context-edge-key"),
  ).toHaveText(firstEdgeKey);
  await expect(
    edgeInspector.getByTestId(
      `project-context-document-body-${CONTEXT_DOCUMENT_A_ID}`,
    ),
  ).toContainText("Only the first Context body is rendered.");

  await edgeInspector
    .getByTestId(`project-context-edge-document-${CONTEXT_DOCUMENT_B_ID}`)
    .click();
  await expect(
    edgeInspector.getByTestId(
      `project-context-document-body-${CONTEXT_DOCUMENT_B_ID}`,
    ),
  ).toContainText("fetched only after switching Documents");
  await expect(
    edgeInspector.getByTestId(
      `project-context-document-body-${CONTEXT_DOCUMENT_A_ID}`,
    ),
  ).toHaveCount(0);
  let bodyCalls = await documentBodyCalls(page);
  expect(
    bodyCalls?.map(
      (call) =>
        (call.payload as { input?: { documentId?: string } }).input?.documentId,
    ),
  ).toEqual([CONTEXT_DOCUMENT_A_ID, CONTEXT_DOCUMENT_B_ID]);
  await waitForAnimations(page);
  await edgeInspector.screenshot({
    path: "test-results/project-context/project-context-edge-inspector-multi-document.png",
  });

  await edgeInspector
    .getByTestId(`project-context-open-document-${CONTEXT_DOCUMENT_B_ID}`)
    .click();
  await expect(page).toHaveURL(
    new RegExp(`/documents\\?document=${CONTEXT_DOCUMENT_B_ID}$`),
  );
  await page.goBack();
  await expect(
    page.getByTestId("project-context-edge-inspector"),
  ).toBeVisible();

  await page.getByTestId("auxiliary-panel-close").click();
  const secondEdgeKey = "2".repeat(64);
  const secondSpokeId = `spoke:${secondEdgeKey}:role:${ROLE_ID}`;
  await page
    .locator(
      `.react-flow__edge[data-id="${secondSpokeId}"] .react-flow__edge-interaction`,
    )
    .dispatchEvent("click");
  const secondEdgeInspector = page.getByTestId(
    "project-context-edge-inspector",
  );
  await expect(
    secondEdgeInspector.getByTestId(
      `project-context-edge-coordinate-document:${CONTEXT_DOCUMENT_A_ID}`,
    ),
  ).toContainText("Architecture record Coordinate");
  await expect(
    secondEdgeInspector.getByTestId(
      `project-context-edge-document-${CONTEXT_DOCUMENT_A_ID}`,
    ),
  ).toHaveCount(0);
  await expect(
    secondEdgeInspector.getByTestId(
      `project-context-edge-document-${CONTEXT_DOCUMENT_C_ID}`,
    ),
  ).toBeVisible();
  await expect(
    secondEdgeInspector.getByTestId(
      `project-context-document-body-${CONTEXT_DOCUMENT_C_ID}`,
    ),
  ).toContainText("second Edge owns this Context Document");

  await secondEdgeInspector
    .getByTestId(
      `project-context-edge-coordinate-document:${CONTEXT_DOCUMENT_A_ID}`,
    )
    .click();
  await expect(
    page.getByTestId("project-context-coordinate-inspector"),
  ).toBeVisible();
  await expect(
    page.getByTestId(`project-context-document-body-${CONTEXT_DOCUMENT_A_ID}`),
  ).toContainText("Only the first Context body is rendered.");
  bodyCalls = await documentBodyCalls(page);
  expect(bodyCalls).toHaveLength(3);
});

test("tombstoned and unavailable Coordinates remain distinct Edge members", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectContext: inspectorResult(),
    projectDocument: inspectorDocumentState(),
    projectView: inspectorProjectView(),
  });
  const tombstoneSelection = encodeURIComponent(`coordinate:goal:${GOAL_ID}`);
  await page.goto(`/#/project-context?selected=${tombstoneSelection}`);
  const inspector = page.getByTestId("project-context-inspector");
  await expect(
    inspector.getByTestId("project-context-coordinate-tombstoned"),
  ).toBeVisible();
  await expect(inspector).toContainText("Known revision");
  await expect(
    inspector.getByTestId(`project-context-coordinate-edge-${"1".repeat(64)}`),
  ).toBeVisible();
  await expect(
    inspector.getByTestId("project-context-open-project-view"),
  ).toHaveCount(0);

  const unavailableSelection = encodeURIComponent(`coordinate:role:${ROLE_ID}`);
  await page.goto(`/#/project-context?selected=${unavailableSelection}`);
  await expect(
    page.getByTestId("project-context-coordinate-unavailable"),
  ).toContainText("Role details are temporarily unavailable.");
  await expect(page.getByText(/Context Gap/i)).toHaveCount(0);
  await expect(
    page.getByTestId(`project-context-coordinate-edge-${"2".repeat(64)}`),
  ).toBeVisible();
  await expect(
    page.getByTestId("project-context-open-project-view"),
  ).toHaveCount(0);
  expect(await documentBodyCalls(page)).toHaveLength(0);
  await waitForAnimations(page);
  await page.getByTestId("project-context-screen").screenshot({
    path: "test-results/project-context/project-context-tombstone-unavailable.png",
  });
});

test("an unavailable Document observation never issues an identity-free body read", async ({
  page,
}) => {
  const result = inspectorResult();
  result.documentObservation = {
    state: "unavailable",
    reason: "Document catalog is reconnecting.",
  };
  await installMockBridge(page, {
    projectContext: result,
    projectDocument: inspectorDocumentState(),
    projectView: inspectorProjectView(),
  });
  await openProjectContext(page);
  expect(await documentBodyCalls(page)).toHaveLength(0);
  await page.getByTestId(`project-context-edge-${"1".repeat(64)}`).click();
  await expect(
    page.getByTestId("project-context-document-source-unavailable"),
  ).toBeVisible();
  await expect(
    page.getByTestId("project-context-edge-inspector"),
  ).toBeVisible();
  expect(await documentBodyCalls(page)).toHaveLength(0);
});

test("a current Document body error does not hide the verified Edge", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectContext: inspectorResult(),
    projectDocument: inspectorDocumentState(),
    projectDocumentReadError: "Current Document verification failed.",
    projectView: inspectorProjectView(),
  });
  await openProjectContext(page);
  await page.getByTestId(`project-context-edge-${"1".repeat(64)}`).click();

  const edgeInspector = page.getByTestId("project-context-edge-inspector");
  await expect(edgeInspector).toBeVisible();
  await expect(
    edgeInspector.locator('[data-testid^="project-context-edge-coordinate-"]'),
  ).toHaveCount(3);
  await expect(
    edgeInspector.getByTestId("project-context-document-error"),
  ).toContainText("Current Document verification failed.");
  await expect(
    edgeInspector.getByTestId("project-context-edge-key"),
  ).toHaveText("1".repeat(64));
  const bodyCalls = await documentBodyCalls(page);
  expect(bodyCalls?.length).toBeGreaterThan(0);
  expect(
    bodyCalls?.every(
      (call) =>
        (call.payload as { input?: { documentId?: string } }).input
          ?.documentId === CONTEXT_DOCUMENT_A_ID,
    ),
  ).toBe(true);
});

test("Query Bar keeps a draft until Run and URL history restores query and selection", async ({
  page,
}) => {
  const incident: ProjectContextQuery = {
    type: "incident",
    coordinate: {
      type: "project_view_object",
      objectType: "requirement",
      objectId: REQUIREMENT_ID,
    },
  };
  await installMockBridge(page, {
    projectContext: contextResult(),
    projectContextsByQuery: {
      [queryKey(incident)]: focusedResult(incident),
    },
  });
  await openProjectContext(page);

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
      ),
    )
    .toBeGreaterThanOrEqual(2);
  const callsBeforeDraft = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
  );

  await page.getByTestId("project-context-mode-incident").click();
  await page.getByTestId("project-context-coordinate-picker").click();
  const search = page.getByTestId("project-context-coordinate-search");
  await search.fill("Verified requirement");
  await search.press("Enter");
  await expect(page.getByTestId("project-context-query-bar")).toHaveAttribute(
    "data-draft-dirty",
    "true",
  );
  expect(
    await page.evaluate(
      () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length,
    ),
  ).toBe(callsBeforeDraft);

  await page.getByTestId("project-context-run-query").click();
  await expect(page).toHaveURL(/mode=incident/);
  await expect(page).toHaveURL(
    new RegExp(`coordinates=requirement(%3A|:)${REQUIREMENT_ID}`),
  );
  await expect(page.getByTestId("project-context-query-summary")).toContainText(
    "1 matching edge",
  );
  await expect(page.getByTestId("project-context-sync-status")).toHaveText(
    "Live",
  );
  const callsAfterRun = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__ ?? [],
  );
  expect(callsAfterRun.length).toBeGreaterThan(callsBeforeDraft);
  for (const call of callsAfterRun.slice(callsBeforeDraft)) {
    expect(call.payload).toMatchObject({ input: { query: incident } });
  }
  await waitForAnimations(page);
  await page.getByTestId("project-context-graph-slot").screenshot({
    path: "test-results/project-context/project-context-incident-anchor.png",
  });
  await page.evaluate(() => {
    window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__ = [];
  });

  await page
    .getByTestId(`project-context-coordinate-requirement:${REQUIREMENT_ID}`)
    .click();
  await expect(page).toHaveURL(/selected=coordinate/);
  expect(
    await page.evaluate(
      () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length,
    ),
  ).toBe(0);

  await page.goBack();
  await expect(page).not.toHaveURL(/selected=/);
  await page.goBack();
  await expect(page).toHaveURL(/#\/project-context$/);
  await expect(
    page.getByTestId("project-context-island-summary"),
  ).toBeVisible();
  await page.goForward();
  await expect(page).toHaveURL(/mode=incident/);
  await expect(page.getByTestId("project-context-query-summary")).toBeVisible();
  await page.goForward();
  await expect(page).toHaveURL(/selected=coordinate/);
});

test("Incident Coordinate selection closes safely and ignores stale repeated input", async ({
  page,
}) => {
  await installMockBridge(page, { projectContext: contextResult() });
  await openProjectContext(page);
  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
      ),
    )
    .toBeGreaterThanOrEqual(2);
  const callsBeforeDraft = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
  );

  await page.getByTestId("project-context-mode-incident").click();
  await page.getByTestId("project-context-coordinate-picker").click();
  await page.evaluate(
    ({ firstKey, secondKey }) => {
      const first = document.querySelector<HTMLElement>(
        `[role="option"][data-coordinate-key="${firstKey}"]`,
      );
      const second = document.querySelector<HTMLElement>(
        `[role="option"][data-coordinate-key="${secondKey}"]`,
      );
      if (!first || !second) throw new Error("Coordinate fixtures missing");
      first.click();
      second.click();
    },
    {
      firstKey: `requirement:${REQUIREMENT_ID}`,
      secondKey: `resource:${RESOURCE_ID}`,
    },
  );

  await expect(
    page.getByTestId("project-context-coordinate-search"),
  ).toBeHidden();
  await expect(
    page.getByTestId("project-context-coordinate-picker"),
  ).toBeDisabled();
  await expect(
    page.getByTestId("project-context-query-chips").locator("li"),
  ).toHaveCount(1);
  await expect(page.getByTestId("project-context-query-chips")).toContainText(
    "Verified requirement",
  );
  await expect(page.getByText("Something went wrong!")).toHaveCount(0);
  expect(
    await page.evaluate(
      () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length ?? 0,
    ),
  ).toBe(callsBeforeDraft);

  await page.getByTestId("project-context-clear-coordinates").click();
  await expect(
    page.getByTestId("project-context-coordinate-picker"),
  ).toBeEnabled();
  await page.getByTestId("project-context-coordinate-picker").click();
  const search = page.getByTestId("project-context-coordinate-search");
  await search.fill("Verified resource");
  await search.evaluate((element) => {
    const init = { bubbles: true, cancelable: true, key: "Enter" };
    element.dispatchEvent(new KeyboardEvent("keydown", init));
    element.dispatchEvent(new KeyboardEvent("keydown", init));
  });
  await expect(search).toBeHidden();
  await expect(
    page.getByTestId("project-context-query-chips").locator("li"),
  ).toHaveCount(1);
  await expect(page.getByText("Something went wrong!")).toHaveCount(0);

  await page.getByTestId("project-context-clear-coordinates").click();
  await page.getByTestId("project-context-mode-exact").click();
  await page.getByTestId("project-context-coordinate-picker").click();
  await expect(
    page.getByTestId("project-context-coordinate-search"),
  ).toBeVisible();
  await page.getByTestId("project-context-mode-all").click();
  await expect(
    page.getByTestId("project-context-coordinate-search"),
  ).toBeHidden();
  await expect(
    page.getByTestId("project-context-coordinate-picker"),
  ).toBeDisabled();
});

test("Exact and Contains all enforce arity and submit canonical typed queries", async ({
  page,
}) => {
  const requirement = {
    type: "project_view_object" as const,
    objectType: "requirement" as const,
    objectId: REQUIREMENT_ID,
  };
  const resource = {
    type: "project_view_object" as const,
    objectType: "resource" as const,
    objectId: RESOURCE_ID,
  };
  const exact: ProjectContextQuery = {
    type: "exact",
    coordinates: [requirement, resource],
  };
  const containsAll: ProjectContextQuery = {
    type: "contains_all",
    coordinates: [requirement],
  };
  await installMockBridge(page, {
    projectContext: contextResult(),
    projectContextsByQuery: {
      [queryKey(exact)]: focusedResult(exact),
      [queryKey(containsAll)]: focusedResult(containsAll),
    },
  });
  await openProjectContext(page);

  await page.getByTestId("project-context-mode-exact").click();
  await page.getByTestId("project-context-coordinate-picker").click();
  await page
    .getByTestId("project-context-coordinate-search")
    .fill("Verified requirement");
  await page.getByTestId("project-context-coordinate-search").press("Enter");
  await expect(page.getByTestId("project-context-run-query")).toBeDisabled();
  await page
    .getByTestId("project-context-coordinate-search")
    .fill("Verified resource");
  await page.getByTestId("project-context-coordinate-search").press("Enter");
  await expect(page.getByTestId("project-context-run-query")).toBeEnabled();
  await page.getByTestId("project-context-run-query").click();
  await expect(page.getByTestId("project-context-query-summary")).toContainText(
    "1 matching edge",
  );

  let calls = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__,
  );
  expect(calls?.at(-1)?.payload).toMatchObject({
    input: { query: exact },
  });

  await page.getByTestId("project-context-mode-contains_all").click();
  await page.getByRole("button", { name: "Remove Verified resource" }).click();
  await page.getByTestId("project-context-run-query").click();
  await expect(page).toHaveURL(/mode=contains_all/);
  calls = await page.evaluate(() => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__);
  expect(calls?.at(-1)?.payload).toMatchObject({
    input: { query: containsAll },
  });

  await page.getByTestId("project-context-mode-all").click();
  await page.getByTestId("project-context-run-query").click();
  await expect(page).toHaveURL(/#\/project-context$/);
  await expect(
    page.getByTestId("project-context-island-summary"),
  ).toBeVisible();
});

test("focused no-match shows Anchors, clears stale selection, and claims no Island or Gap", async ({
  page,
}) => {
  const incident: ProjectContextQuery = {
    type: "incident",
    coordinate: { type: "document", documentId: DOCUMENT_COORDINATE_ID },
  };
  await installMockBridge(page, {
    projectContext: contextResult(),
    projectContextsByQuery: {
      [queryKey(incident)]: focusedResult(incident, { noMatch: true }),
    },
  });
  await page.goto(
    `/#/project-context?mode=incident&coordinates=document:${DOCUMENT_COORDINATE_ID}&selected=edge:${"1".repeat(64)}`,
  );

  await expect(page.getByTestId("project-context-query-summary")).toContainText(
    "0 matching edges",
  );
  await expect(
    page.getByTestId(
      `project-context-coordinate-document:${DOCUMENT_COORDINATE_ID}`,
    ),
  ).toHaveAttribute("data-query-anchor", "true");
  await expect(page.getByTestId("project-context-island-summary")).toHaveCount(
    0,
  );
  await expect(page.getByText(/Context Gap/i)).toHaveCount(0);
  await expect(page).not.toHaveURL(/selected=/);
});

test("invalid copied route is rejected before the trusted query boundary", async ({
  page,
}) => {
  await installMockBridge(page, { projectContext: contextResult() });
  await page.goto("/#/project-context?mode=exact&coordinates=not-a-coordinate");
  await expect(page.getByTestId("project-context-invalid-route")).toBeVisible();
  expect(
    await page.evaluate(
      () => window.__BUZZ_E2E_PROJECT_CONTEXT_CALLS__?.length,
    ),
  ).toBe(0);
  await page.getByTestId("project-context-reset-invalid-route").click();
  await expect(page).toHaveURL(/#\/project-context$/);
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
});

test("All Context renders binary, hyperedge overlap, and two labelled Islands", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await installMockBridge(page, { projectContext: twoIslandResult() });
  await openProjectContext(page);

  await expect(
    page.getByTestId("project-context-island-summary"),
  ).toContainText(
    "2 context islands · 5 coordinates · 3 edges · 3 context docs",
  );
  await expect(page.getByText("2 disconnected components")).toBeVisible();
  await expect(
    page.locator('[data-testid^="project-context-island-"][data-island]'),
  ).toHaveCount(2);
  await expect(page.getByTestId("project-context-island-1")).toContainText(
    "3 coordinates · 2 edges · 2 context docs",
  );
  await expect(page.getByTestId("project-context-island-2")).toContainText(
    "2 coordinates · 1 edge · 1 context doc",
  );
  const firstIsland = page.getByTestId("project-context-island-1");
  const secondIsland = page.getByTestId("project-context-island-2");
  const [firstHue, secondHue, lightBorder] = await Promise.all([
    firstIsland.evaluate((element) =>
      getComputedStyle(element).getPropertyValue(
        "--project-context-island-hue",
      ),
    ),
    secondIsland.evaluate((element) =>
      getComputedStyle(element).getPropertyValue(
        "--project-context-island-hue",
      ),
    ),
    firstIsland.evaluate((element) => getComputedStyle(element).borderColor),
  ]);
  expect(firstHue).not.toBe(secondHue);
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await expect
    .poll(() =>
      firstIsland.evaluate((element) => getComputedStyle(element).borderColor),
    )
    .not.toBe(lightBorder);
  expect(
    await firstIsland.evaluate((element) =>
      getComputedStyle(element).getPropertyValue(
        "--project-context-island-hue",
      ),
    ),
  ).toBe(firstHue);
  await page.evaluate(() => document.documentElement.classList.remove("dark"));
  await expect(
    page.locator('[data-testid^="project-context-edge-"]'),
  ).toHaveCount(3);
  await expect(page.locator(".project-context-spoke")).toHaveCount(7);
  await expect(page.locator(".project-context-coordinate")).toHaveCount(5);

  await expect(
    page.getByTestId(`project-context-coordinate-resource:${RESOURCE_ID}`),
  ).toHaveAttribute("data-lifecycle", "tombstoned");
  await expect(
    page.getByTestId(`project-context-coordinate-role:${ROLE_ID}`),
  ).toHaveAttribute("data-lifecycle", "unavailable");
  await expect(
    page.getByTestId(
      `project-context-coordinate-document:${CONTEXT_DOCUMENT_A_ID}`,
    ),
  ).toHaveCount(1);
  await expect(
    page.getByTestId(
      `project-context-coordinate-document:${CONTEXT_DOCUMENT_B_ID}`,
    ),
  ).toHaveCount(0);

  await page.getByTestId(`project-context-edge-${"2".repeat(64)}`).click();
  await expect(
    page.getByTestId(`project-context-edge-${"2".repeat(64)}`),
  ).toHaveAttribute("data-emphasis", "active");
  await expect(
    page.getByTestId(`project-context-edge-${"1".repeat(64)}`),
  ).toHaveAttribute("data-emphasis", "dimmed");
  const selectedSpokes = page.locator(
    `.project-context-spoke[data-edge-key="${"2".repeat(64)}"]`,
  );
  await expect(selectedSpokes).toHaveCount(3);
  for (let index = 0; index < 3; index += 1) {
    await expect(selectedSpokes.nth(index)).toHaveAttribute(
      "data-emphasis",
      "active",
    );
  }
  const overlapSpokes = page.locator(
    `.project-context-spoke[data-edge-key="${"1".repeat(64)}"]`,
  );
  await expect(overlapSpokes).toHaveCount(2);
  for (let index = 0; index < 2; index += 1) {
    await expect(overlapSpokes.nth(index)).toHaveAttribute(
      "data-emphasis",
      "dimmed",
    );
  }
  await expect(
    page.getByTestId("project-context-selection-status"),
  ).toContainText("3 coordinates · 1 doc");
  await waitForAnimations(page);
  await page.getByTestId("project-context-graph-slot").screenshot({
    path: "test-results/project-context/project-context-overlapping-edges.png",
  });

  const binarySpokeId = `spoke:${"1".repeat(64)}:requirement:${REQUIREMENT_ID}`;
  const binarySpoke = page.locator(
    `.react-flow__edge[data-id="${binarySpokeId}"] .react-flow__edge-interaction`,
  );
  await expect(binarySpoke).toHaveCount(1);
  await binarySpoke.dispatchEvent("click");
  await expect(
    page.getByTestId(`project-context-edge-${"1".repeat(64)}`),
  ).toHaveAttribute("data-emphasis", "active");
  for (let index = 0; index < 2; index += 1) {
    await expect(overlapSpokes.nth(index)).toHaveAttribute(
      "data-emphasis",
      "active",
    );
  }
  await expect(
    page.getByTestId("project-context-selection-status"),
  ).toContainText("2 coordinates · 1 doc");

  await page.keyboard.press("Escape");

  await page.getByTestId("project-context-fit-island-2").click();
  await page.getByTestId("project-context-fit-all").click();
  await page.waitForTimeout(300);
  await waitForAnimations(page);
  await page.getByTestId("project-context-graph-slot").screenshot({
    path: "test-results/project-context/project-context-two-islands.png",
  });

  await page.evaluate(() => {
    window.localStorage.setItem("buzz-theme", "buzz-dark");
  });
  await page.reload();
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
  await expect(page.locator("html")).toHaveClass(/dark/);
  await waitForAnimations(page);
  await page.getByTestId("project-context-graph-slot").screenshot({
    path: "test-results/project-context/project-context-dark-islands.png",
  });
});

test("live Context hints merge and split Islands only after a trusted replacement", async ({
  page,
}) => {
  const split = twoIslandResult();
  await installMockBridge(page, { projectContext: split });
  await openProjectContext(page);
  await waitForProjectContextLive(page);
  await expect(
    page.getByTestId("project-context-island-summary"),
  ).toContainText("2 context islands");

  const merged = structuredClone(split);
  merged.context = {
    ...merged.context,
    contextRevision: 8,
    activeEdgeCount: 4,
    boundDocumentCount: 4,
    updatedAt: "2026-08-06T08:01:00Z",
    metaEventId: "8".repeat(64),
  };
  merged.edges.push({
    edgeKey: "4".repeat(64),
    coordinateKeys: [`requirement:${REQUIREMENT_ID}`, `role:${ROLE_ID}`],
    contextDocumentIds: [CONTEXT_DOCUMENT_D_ID],
  });
  merged.documentDetails.push({
    documentId: CONTEXT_DOCUMENT_D_ID,
    state: "active",
    title: "Cross-Island rationale",
    documentRevision: 1,
  });

  await page.evaluate(
    ({ kind, result }) => {
      window.__BUZZ_E2E_SET_PROJECT_CONTEXT__?.(result);
      window.__BUZZ_E2E_EMIT_PROJECT_CONTEXT_EVENT__?.({ kind });
    },
    { kind: KIND_PROJECT_CONTEXT_EDGE_BINDING, result: merged },
  );
  await expect(
    page.getByTestId("project-context-island-summary"),
  ).toContainText(
    "1 context island · 5 coordinates · 4 edges · 4 context docs",
  );
  await expect(
    page.getByText("All visible Context Edges form one"),
  ).toBeVisible();
  await expect(
    page.getByText("untrusted-live-context-must-not-render"),
  ).toHaveCount(0);

  const splitAgain = structuredClone(split);
  splitAgain.context = {
    ...splitAgain.context,
    contextRevision: 9,
    updatedAt: "2026-08-06T08:02:00Z",
    metaEventId: "9".repeat(64),
  };
  await page.evaluate(
    ({ kind, result }) => {
      window.__BUZZ_E2E_SET_PROJECT_CONTEXT__?.(result);
      window.__BUZZ_E2E_EMIT_PROJECT_CONTEXT_EVENT__?.({ kind });
    },
    { kind: KIND_PROJECT_CONTEXT_EDGE_BINDING, result: splitAgain },
  );
  await expect(
    page.getByTestId("project-context-island-summary"),
  ).toContainText(
    "2 context islands · 5 coordinates · 3 edges · 3 context docs",
  );
});

test("live Project View hints refresh Coordinate title and detail through verified reads", async ({
  page,
}) => {
  const initialContext = inspectorResult();
  await installMockBridge(page, {
    projectContext: initialContext,
    projectDocument: inspectorDocumentState(),
    projectView: inspectorProjectView(),
  });
  await openProjectContext(page);
  await waitForProjectContextLive(page);
  await page
    .getByTestId(`project-context-coordinate-requirement:${REQUIREMENT_ID}`)
    .click();

  const nextView = structuredClone(inspectorProjectView());
  if (nextView.status !== "ready") throw new Error("Expected ready fixture");
  nextView.project_revision = 12;
  nextView.updated_at = "2026-08-06T08:01:00Z";
  const requirement = nextView.objects_v3.find(
    (object) => object.id === REQUIREMENT_ID,
  );
  if (requirement?.data.object_type !== "requirement") {
    throw new Error("Expected requirement fixture");
  }
  requirement.object_revision = 5;
  requirement.project_revision = 12;
  requirement.updated_at = "2026-08-06T08:01:00Z";
  requirement.data.data.title = "Live verified requirement detail";
  requirement.data.data.description =
    "This detail appeared only after the verified Project View re-read.";

  const nextContext = structuredClone(initialContext);
  nextContext.projectViewObservation = {
    state: "observed",
    projectRevision: 12,
    projectionGeneration: 3,
    updatedAt: "2026-08-06T08:01:00Z",
    metaEventId: "5".repeat(64),
  };
  const contextRequirement = nextContext.coordinateDetails.find(
    (detail) => detail.coordinateKey === `requirement:${REQUIREMENT_ID}`,
  );
  if (!contextRequirement) throw new Error("Expected Context Coordinate");
  contextRequirement.title = "Live requirement Coordinate";
  contextRequirement.objectRevision = 5;
  contextRequirement.updatedAt = "2026-08-06T08:01:00Z";

  await page.evaluate(
    ({ context, kind, view }) => {
      window.__BUZZ_E2E_SET_PROJECT_VIEW__?.(view);
      window.__BUZZ_E2E_SET_PROJECT_CONTEXT__?.(context);
      window.__BUZZ_E2E_EMIT_PROJECT_VIEW_EVENT__?.({ kind });
    },
    { context: nextContext, kind: KIND_PROJECT_VIEW_OBJECT, view: nextView },
  );

  await expect(
    page.getByTestId(
      `project-context-coordinate-requirement:${REQUIREMENT_ID}`,
    ),
  ).toContainText("Live requirement Coordinate");
  await expect(
    page.getByTestId("project-context-project-view-detail"),
  ).toContainText("Live verified requirement detail");
  await expect(page.getByTestId("project-context-inspector")).toContainText(
    "This detail appeared only after the verified Project View re-read.",
  );
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "2");
});

test("live Document hints refresh the selected current body without changing topology", async ({
  page,
}) => {
  const initialContext = inspectorResult();
  const initialDocuments = inspectorDocumentState();
  await installMockBridge(page, {
    projectContext: initialContext,
    projectDocument: initialDocuments,
    projectView: inspectorProjectView(),
  });
  await openProjectContext(page);
  await waitForProjectContextLive(page);
  await page.getByTestId(`project-context-edge-${"1".repeat(64)}`).click();
  await expect(
    page.getByTestId(`project-context-document-body-${CONTEXT_DOCUMENT_A_ID}`),
  ).toContainText("Only the first Context body is rendered.");

  const nextDocuments = structuredClone(initialDocuments);
  const nextBody = contextDocumentSnapshot({
    contentMarkdown:
      "# Live architecture rationale A\n\nThe current body changed without changing Edge membership.",
    documentId: CONTEXT_DOCUMENT_A_ID,
    eventDigit: "9",
    revision: 9,
    summary: "Live verified summary for the architecture rationale.",
    title: "Live architecture rationale",
  });
  nextDocuments.meta = {
    ...nextDocuments.meta,
    catalogRevision: 9,
    updatedAt: "2026-08-06T08:01:00Z",
    metaEventId: "9".repeat(64),
  };
  const documentIndex = nextDocuments.documents.findIndex(
    (document) => document.documentId === CONTEXT_DOCUMENT_A_ID,
  );
  nextDocuments.documents[documentIndex] = {
    documentId: nextBody.documentId,
    title: nextBody.title ?? "Context Document",
    summary: nextBody.summary,
    documentRevision: nextBody.documentRevision,
    updatedAt: nextBody.revisionAt,
    updatedBy: nextBody.revisionBy,
    headEventId: nextBody.headEventId ?? nextBody.revisionEventId,
  };
  nextDocuments.revisions[CONTEXT_DOCUMENT_A_ID] = [
    ...(nextDocuments.revisions[CONTEXT_DOCUMENT_A_ID] ?? []),
    nextBody,
  ];

  const nextContext = structuredClone(initialContext);
  nextContext.documentObservation = {
    state: "observed",
    catalogRevision: 9,
    projectionGeneration: 2,
    updatedAt: "2026-08-06T08:01:00Z",
    metaEventId: "9".repeat(64),
  };
  const contextDocument = nextContext.documentDetails.find(
    (document) => document.documentId === CONTEXT_DOCUMENT_A_ID,
  );
  if (!contextDocument) throw new Error("Expected Context Document detail");
  contextDocument.title = "Live architecture rationale";
  contextDocument.summary =
    "Live verified summary for the architecture rationale.";
  contextDocument.documentRevision = 9;
  contextDocument.updatedAt = "2026-08-06T08:01:00Z";

  await page.evaluate(
    ({ context, documents, kind }) => {
      window.__BUZZ_E2E_SET_PROJECT_DOCUMENT_STATE__?.(documents);
      window.__BUZZ_E2E_SET_PROJECT_CONTEXT__?.(context);
      window.__BUZZ_E2E_EMIT_PROJECT_DOCUMENT_EVENT__?.({ kind });
    },
    {
      context: nextContext,
      documents: nextDocuments,
      kind: KIND_PROJECT_DOCUMENT_HEAD,
    },
  );

  await expect(
    page.getByTestId(`project-context-document-body-${CONTEXT_DOCUMENT_A_ID}`),
  ).toContainText("The current body changed without changing Edge membership.");
  await expect(
    page.getByTestId("project-context-edge-inspector"),
  ).toContainText("Live architecture rationale");
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "2");
  await expect(
    page
      .getByTestId("project-context-edge-inspector")
      .locator('[data-testid^="project-context-edge-coordinate-"]'),
  ).toHaveCount(3);
});

test("offline Context stays visible as stale and reconnect replaces the whole trusted snapshot", async ({
  page,
}) => {
  const initial = contextResult({ edgeCount: 1, revision: 7 });
  await installMockBridge(page, { projectContext: initial });
  await openProjectContext(page);
  await waitForProjectContextLive(page);

  await page.evaluate(() => {
    window.__BUZZ_E2E_SET_RELAY_CONNECTION_STATE__?.("disconnected");
  });
  await expect(page.getByText("Reconnecting", { exact: true })).toBeVisible();
  await expect(page.getByTestId("project-context-stale-message")).toContainText(
    "It may be stale while the Relay connection recovers.",
  );
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "1");

  const recovered = contextResult({ edgeCount: 2, revision: 8 });
  recovered.context.updatedAt = "2026-08-06T08:01:00Z";
  recovered.context.metaEventId = "8".repeat(64);
  await page.evaluate((result) => {
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT__?.(result);
    window.__BUZZ_E2E_SET_RELAY_CONNECTION_STATE__?.("connected");
  }, recovered);

  await expect(page.getByText("Revision 8", { exact: true })).toBeVisible();
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "2");
  await expect(page.getByTestId("project-context-stale-message")).toHaveCount(
    0,
  );
});

test("a verification failure after a trusted graph fails closed and can be verified again", async ({
  page,
}) => {
  await installMockBridge(page, { projectContext: contextResult() });
  await openProjectContext(page);
  await waitForProjectContextLive(page);

  await page.evaluate(() => {
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT_ERROR__?.({
      code: "verification_failed",
      message: "The replacement projection failed verification.",
      retryable: false,
    });
  });
  await page.getByTestId("project-context-refresh").click();
  await expect(
    page.getByTestId("project-context-verification-failed"),
  ).toBeVisible();
  await expect(page.getByTestId("project-context-graph-slot")).toHaveCount(0);
  await expect(page.getByText("Stale", { exact: true })).toHaveCount(0);

  const repaired = contextResult({ edgeCount: 2, revision: 8 });
  repaired.context.updatedAt = "2026-08-06T08:01:00Z";
  await page.evaluate((result) => {
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT_ERROR__?.();
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT__?.(result);
  }, repaired);
  await page.getByRole("button", { name: "Verify again" }).click();
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
  await expect(page.getByText("Revision 8", { exact: true })).toBeVisible();
});

test("keyboard selection, resizable Inspector, reduced motion, and text zoom remain independent", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockBridge(page, {
    projectContext: inspectorResult(),
    projectDocument: inspectorDocumentState(),
    projectView: inspectorProjectView(),
  });
  await openProjectContext(page);

  const requirementNode = page.getByTestId(
    `project-context-coordinate-requirement:${REQUIREMENT_ID}`,
  );
  const requirementButton = requirementNode.locator("button");
  await requirementButton.focus();
  await page.keyboard.press("Space");
  await expect(page.getByTestId("project-context-inspector")).toBeVisible();
  await expect(requirementButton).toHaveAttribute("aria-pressed", "true");
  await page.getByTestId("project-context-fit-selection").click();
  await expect(requirementButton).toBeFocused();

  const inspector = page.getByTestId("project-context-inspector");
  const initialInspectorWidth = (await inspector.boundingBox())?.width ?? 0;
  const resizeHandle = page.getByTestId(
    "project-context-inspector-resize-handle",
  );
  const handleBox = await resizeHandle.boundingBox();
  if (!handleBox) throw new Error("Inspector resize handle is unavailable");
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + 80);
  await page.mouse.down();
  await page.mouse.move(handleBox.x - 100, handleBox.y + 80);
  await page.mouse.up();
  await expect
    .poll(async () => (await inspector.boundingBox())?.width ?? 0)
    .toBeGreaterThan(initialInspectorWidth + 80);
  await resizeHandle.dblclick();
  await expect
    .poll(async () => Math.round((await inspector.boundingBox())?.width ?? 0))
    .toBe(440);

  await page.keyboard.press("Escape");
  await page.getByTestId("project-context-fit-all-canvas").click();
  const edgeButton = page
    .getByTestId(`project-context-edge-${"1".repeat(64)}`)
    .locator("button");
  await expect(edgeButton).toHaveAttribute(
    "aria-label",
    "Context Edge connecting 3 coordinates with 2 documents",
  );
  await edgeButton.focus();
  await page.keyboard.press("Enter");
  await expect(
    page.getByTestId("project-context-edge-inspector"),
  ).toBeVisible();
  await expect(edgeButton).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("Escape");

  const coordinateFlowNode = page.locator(
    `.react-flow__node[data-id="coordinate:requirement:${REQUIREMENT_ID}"]`,
  );
  const widthBeforeTextZoom = await coordinateFlowNode.evaluate((element) =>
    Number.parseFloat(getComputedStyle(element).width),
  );
  await page.evaluate(() => {
    document.documentElement.style.fontSize = "20px";
  });
  await expect
    .poll(() =>
      coordinateFlowNode.evaluate((element) =>
        Number.parseFloat(getComputedStyle(element).width),
      ),
    )
    .toBeGreaterThan(widthBeforeTextZoom);

  const rootFontSize = await page.evaluate(
    () => getComputedStyle(document.documentElement).fontSize,
  );
  await page.getByRole("button", { name: "Zoom in" }).click();
  expect(
    await page.evaluate(
      () => getComputedStyle(document.documentElement).fontSize,
    ),
  ).toBe(rootFontSize);
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

test("sequenced refresh failure keeps stale data and the next read recovers", async ({
  page,
}) => {
  await installMockBridge(page, { projectContext: contextResult() });
  await page.goto("/#/project-context");
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
  const recovered = contextResult({ edgeCount: 2, revision: 9 });
  await page.evaluate((result) => {
    window.__BUZZ_E2E_SET_PROJECT_CONTEXT_READ_SEQUENCE__?.([
      {
        delayMs: 25,
        error: {
          code: "unavailable",
          message: "Relay temporarily unavailable.",
          retryable: true,
        },
      },
      { delayMs: 25, result },
    ]);
  }, recovered);
  await page.getByTestId("project-context-refresh").click();
  await expect(page.getByTestId("project-context-stale-message")).toContainText(
    "Relay temporarily unavailable",
  );
  await expect(page.getByTestId("project-context-graph-slot")).toBeVisible();
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "1");

  await page.getByTestId("project-context-refresh").click();
  await expect(page.getByText("Revision 9", { exact: true })).toBeVisible();
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "2");
  await expect(page.getByTestId("project-context-stale-message")).toHaveCount(
    0,
  );
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
        [COMMUNITY_A.relayUrl]: 250,
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

  await page
    .getByTestId(`project-context-coordinate-requirement:${REQUIREMENT_ID}`)
    .click();
  await expect(page).toHaveURL(/selected=coordinate/);
  await page.getByTestId(`community-rail-button-${COMMUNITY_A.id}`).click();
  await page.getByTestId("open-project-context").click();
  await expect(page).toHaveURL(/#\/project-context$/);
  await expect(page.getByTestId("project-context-loading")).toBeVisible();
  await expect(page.getByTestId("project-context-result-counts")).toHaveCount(
    0,
  );
  await expect(
    page.getByTestId("project-context-result-counts"),
  ).toHaveAttribute("data-edge-count", "1");
  await expect(page).not.toHaveURL(/selected=/);
});
