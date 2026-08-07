import { expect, test } from "@playwright/test";

import type {
  RawProjectViewLoadResult,
  RawProjectViewObjectV3,
  ProjectViewObjectType,
} from "../../src/shared/api/tauriProjectView";
import type { ProjectContextQueryResult } from "../../src/shared/api/tauriProjectContext";
import type {
  ProjectDocument,
  ProjectDocumentMeta,
} from "../../src/shared/api/tauriProjectDocument";
import type { RawProjectRoleHistoryPage } from "../../src/shared/api/tauriProjectViewRoleHistory";
import { KIND_PROJECT_VIEW_OBJECT } from "../../src/shared/constants/kinds";
import {
  PROJECT_VIEW_V3_ENTITY_TAG,
  PROJECT_VIEW_V3_OBJECT_TAG,
} from "../../src/shared/constants/projectView";
import type { MockProjectDocumentState } from "../../src/testing/e2eBridge";
import { installMockBridge } from "../helpers/bridge";
import { openSettings } from "../helpers/settings";

const ACTOR = "a".repeat(64);
const HUMAN = "deadbeef".repeat(8);
const FORMER_ASSIGNEE = "e".repeat(64);
const ROLE_CANDIDATE = "9".repeat(64);
const ALICE =
  "953d3363262e86b770419834c53d2446409db6d918a57f8f339d495d54ab001f";
const NOW = "2026-07-27T08:00:00Z";
const GUIDE_ID = "30000000-0000-4000-8000-000000000001";
const COMMUNITY_A = {
  id: "project-view-a",
  name: "Alpha",
  relayUrl: "ws://localhost:3000",
  addedAt: "2026-07-27T00:00:00.000Z",
};
const COMMUNITY_B = {
  id: "project-view-b",
  name: "Bravo",
  relayUrl: "ws://localhost:3001",
  addedAt: "2026-07-27T00:01:00.000Z",
};
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
): RawProjectViewObjectV3 {
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
    context_references: [],
  } as RawProjectViewObjectV3;
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
  resource_kind: "repository",
  summary: "Source repository for the Buzz implementation.",
  guide_document_id: GUIDE_ID,
});

const ROLE_STATE_IDS = {
  currentAssignment: "20000000-0000-4000-8000-000000000001",
  formerAssignment: "20000000-0000-4000-8000-000000000002",
  proposal: "20000000-0000-4000-8000-000000000003",
  handoff: "20000000-0000-4000-8000-000000000004",
  commitment: "20000000-0000-4000-8000-000000000005",
  checkpoint: "20000000-0000-4000-8000-000000000006",
} as const;

const ROLE_BRIEF_SOURCE = {
  event_id: "d".repeat(64),
  project_revision: 7,
  item_revision: 1,
  change_id: "e".repeat(64),
  source_type: "nostr_event",
};

const ROLE_CONTINUITY_V3 = {
  roles: [
    {
      role_id: IDS.role,
      name: "Context steward",
      purpose: "Keep project intent coherent.",
      responsibilities: ["Review project structure"],
      boundaries: ["Does not grant unscoped authority"],
      level: "admin",
      active: true,
      object_revision: 1,
      project_revision: 7,
      created_at: NOW,
      updated_at: NOW,
      created_by: ACTOR,
      updated_by: ACTOR,
      context_references: [],
    },
  ],
  proposals: [
    {
      proposal_id: ROLE_STATE_IDS.proposal,
      role_id: IDS.role,
      candidate_pubkey: FORMER_ASSIGNEE,
      proposal_type: "request",
      candidate_accepted_at: NOW,
      expected_target_assignment_id: ROLE_STATE_IDS.currentAssignment,
      expires_at: "2026-08-01T08:00:00Z",
      status: "open",
      reason: "Return to project context stewardship.",
      created_by: FORMER_ASSIGNEE,
      created_at: NOW,
      entity_revision: 1,
      project_revision: 7,
    },
  ],
  assignments: [
    {
      assignment_id: ROLE_STATE_IDS.currentAssignment,
      role_id: IDS.role,
      member_pubkey: ACTOR,
      started_at: NOW,
      started_by: HUMAN,
      entity_revision: 1,
      project_revision: 7,
    },
    {
      assignment_id: ROLE_STATE_IDS.formerAssignment,
      role_id: IDS.role,
      member_pubkey: FORMER_ASSIGNEE,
      started_at: "2026-07-20T08:00:00Z",
      started_by: HUMAN,
      ended_at: "2026-07-26T08:00:00Z",
      ended_by: HUMAN,
      ended_reason: "replaced",
      replaced_by_assignment_id: ROLE_STATE_IDS.currentAssignment,
      entity_revision: 2,
      project_revision: 6,
    },
  ],
  commitments: [
    {
      commitment_id: ROLE_STATE_IDS.commitment,
      work_id: IDS.work,
      assignment_id: ROLE_STATE_IDS.currentAssignment,
      member_pubkey: ACTOR,
      started_at: NOW,
      started_by: ACTOR,
      entity_revision: 1,
      project_revision: 7,
    },
  ],
  workResponsibilities: [{ workId: IDS.work, roleId: IDS.role }],
  checkpoints: [
    {
      checkpoint_id: ROLE_STATE_IDS.checkpoint,
      role_id: IDS.role,
      assignment_id: ROLE_STATE_IDS.currentAssignment,
      based_on_project_revision: 6,
      content: {
        summary: "The View is usable; finish the continuity timeline.",
        current_focus: ["Role continuity timeline"],
        progress: ["Trusted Role Brief is visible"],
        blockers: [],
        risks: ["A stale projection could hide new context"],
        open_questions: ["How much history should load initially?"],
        next_steps: ["Ship paginated Checkpoint history"],
        references: [
          {
            reference_type: "object",
            object_id: IDS.work,
            label: "Add the View entry",
          },
        ],
      },
      created_by: ACTOR,
      created_at: NOW,
      entity_revision: 1,
      project_revision: 7,
    },
  ],
  handoffs: [
    {
      handoff_id: ROLE_STATE_IDS.handoff,
      role_id: IDS.role,
      from_assignment_id: ROLE_STATE_IDS.formerAssignment,
      to_assignment_id: ROLE_STATE_IDS.currentAssignment,
      affected_commitment_ids: [],
      content: {
        summary: "Continue from the verified Project View.",
        unresolved_items: ["Keep Role context current"],
        references: [],
      },
      cause: "replaced",
      system_generated: true,
      created_at: "2026-07-26T08:00:00Z",
      entity_revision: 1,
      project_revision: 6,
    },
  ],
  members: [
    { pubkey: HUMAN, role: "owner" },
    { pubkey: ACTOR, role: "admin" },
    { pubkey: FORMER_ASSIGNEE, role: "member" },
  ],
  briefs: [
    {
      project_view_schema_version: 3,
      generated_at: NOW,
      project_id: IDS.profile,
      project_revision: 7,
      projection_generation: 3,
      member_pubkey: ACTOR,
      community_role: "admin",
      project: {
        profile: {
          object: profile,
          source: ROLE_BRIEF_SOURCE,
        },
        goals: [
          {
            object: goal,
            source: ROLE_BRIEF_SOURCE,
          },
        ],
      },
      role_directory: {
        total_active_roles: 1,
        entries: [
          {
            role_id: IDS.role,
            name: "Context steward",
            level: "admin",
            purpose_summary: "Keep project intent coherent.",
            assignment: {
              status: "assigned",
              assignment_id: ROLE_STATE_IDS.currentAssignment,
              member_pubkey: ACTOR,
              source: ROLE_BRIEF_SOURCE,
            },
            is_current_member_role: true,
            role_source: ROLE_BRIEF_SOURCE,
          },
        ],
        omitted_active_roles: 0,
      },
      state: {
        status: "assigned",
        role: {
          role: {
            role_id: IDS.role,
            name: "Context steward",
            purpose: "Keep project intent coherent.",
            responsibilities: ["Review project structure"],
            boundaries: ["Does not grant unscoped authority"],
            level: "admin",
            active: true,
            object_revision: 1,
            project_revision: 7,
            created_at: NOW,
            updated_at: NOW,
            created_by: ACTOR,
            updated_by: ACTOR,
            context_references: [],
          },
          source: ROLE_BRIEF_SOURCE,
        },
        assignment: {
          assignment: {
            assignment_id: ROLE_STATE_IDS.currentAssignment,
            role_id: IDS.role,
            member_pubkey: ACTOR,
            proposal_id: ROLE_STATE_IDS.proposal,
            started_at: NOW,
            started_by: HUMAN,
            entity_revision: 1,
            project_revision: 7,
          },
          source: ROLE_BRIEF_SOURCE,
        },
      },
      responsible_work: [
        {
          work: {
            object: work,
            responsible_role_id: IDS.role,
            source: ROLE_BRIEF_SOURCE,
          },
          state: {
            status: "committed",
            commitment: {
              commitment: {
                commitment_id: ROLE_STATE_IDS.commitment,
                work_id: IDS.work,
                assignment_id: ROLE_STATE_IDS.currentAssignment,
                member_pubkey: ACTOR,
                started_at: NOW,
                started_by: ACTOR,
                entity_revision: 1,
                project_revision: 7,
              },
              source: ROLE_BRIEF_SOURCE,
            },
          },
        },
      ],
      related_objects: [],
      latest_checkpoint: {
        checkpoint: {
          checkpoint_id: ROLE_STATE_IDS.checkpoint,
          role_id: IDS.role,
          assignment_id: ROLE_STATE_IDS.currentAssignment,
          based_on_project_revision: 6,
          content: {
            summary: "The View is usable; finish the continuity timeline.",
            current_focus: ["Role continuity timeline"],
            progress: ["Trusted Role Brief is visible"],
            blockers: [],
            risks: ["A stale projection could hide new context"],
            open_questions: ["How much history should load initially?"],
            next_steps: ["Ship paginated Checkpoint history"],
            references: [
              {
                reference_type: "object",
                object_id: IDS.work,
                label: "Add the View entry",
              },
            ],
          },
          created_by: ACTOR,
          created_at: NOW,
          entity_revision: 1,
          project_revision: 7,
        },
        source: ROLE_BRIEF_SOURCE,
      },
      recent_handoffs: [
        {
          handoff: {
            handoff_id: ROLE_STATE_IDS.handoff,
            role_id: IDS.role,
            from_assignment_id: ROLE_STATE_IDS.formerAssignment,
            to_assignment_id: ROLE_STATE_IDS.currentAssignment,
            affected_commitment_ids: [],
            content: {
              summary: "Continue from the verified Project View.",
              unresolved_items: ["Keep Role context current"],
              references: [],
            },
            cause: "replaced",
            system_generated: true,
            created_at: "2026-07-26T08:00:00Z",
            entity_revision: 1,
            project_revision: 6,
          },
          source: ROLE_BRIEF_SOURCE,
        },
      ],
      source_revisions: {
        meta_event_id: "f".repeat(64),
        meta_change_id: "a".repeat(64),
        membership_event_id: "c".repeat(64),
        project_updated_at: NOW,
        document_metadata: { state: "not_required" },
      },
      context: {
        availability: { state: "not_advertised_empty" },
        resources: [],
        live_documents: [],
        pinned_documents: [],
        truncation: {
          truncated: false,
          omitted_resources: 0,
          omitted_live_documents: 0,
          omitted_pinned_documents: 0,
        },
      },
    },
  ],
} satisfies Extract<
  RawProjectViewLoadResult,
  { status: "ready" }
>["role_continuity"];

const V3_READY_VIEW = {
  status: "ready",
  relay_pubkey: "b".repeat(64),
  schema_version: 3,
  project_revision: 7,
  projection_generation: 3,
  active_object_count: 10,
  updated_at: NOW,
  objects_v3: [
    profile,
    goal,
    plan,
    stage,
    requirement,
    issue,
    work,
    looseIssue,
    role,
    resource,
  ],
  role_continuity: structuredClone(ROLE_CONTINUITY_V3),
} satisfies RawProjectViewLoadResult;

const READY_VIEW = V3_READY_VIEW;

function contextReadyV3View(): RawProjectViewLoadResult {
  return {
    ...structuredClone(V3_READY_VIEW),
    project_context_supported: true,
  };
}

function contextUnavailableWithPreservedIssueRef(): RawProjectViewLoadResult {
  const view = structuredClone(V3_READY_VIEW);
  const issueObject = view.objects_v3.find(
    (candidate) => candidate.id === IDS.issue,
  );
  if (!issueObject) throw new Error("v3 Issue fixture is unavailable");
  issueObject.context_references = [
    { type: "resource", resource_id: IDS.resource },
  ];
  return view;
}

function guideDocumentState(): MockProjectDocumentState {
  const guide: ProjectDocument = {
    communityKey: "fixture",
    projectId: "40000000-0000-4000-8000-000000000001",
    relayPubkey: "b".repeat(64),
    projectionGeneration: 3,
    documentId: GUIDE_ID,
    documentRevision: 2,
    state: "active",
    title: "Buzz repository guide",
    summary: "Verified repository entry point.",
    contentMarkdown:
      "# Buzz repository\n\nUse the reviewed clone and contribution workflow.",
    createdAt: "2026-07-26T08:00:00Z",
    createdBy: HUMAN,
    revisionAt: NOW,
    revisionBy: HUMAN,
    revisionEventId: "1".repeat(64),
    headEventId: "2".repeat(64),
    sourceEventId: "3".repeat(64),
  };
  const meta: ProjectDocumentMeta = {
    communityKey: "fixture",
    projectId: guide.projectId,
    relayPubkey: guide.relayPubkey,
    projectionGeneration: guide.projectionGeneration,
    catalogRevision: 2,
    activeDocumentCount: 1,
    updatedAt: NOW,
    metaEventId: "4".repeat(64),
  };
  return {
    meta,
    documents: [
      {
        documentId: GUIDE_ID,
        title: guide.title ?? "Buzz repository guide",
        summary: guide.summary,
        documentRevision: guide.documentRevision,
        updatedAt: guide.revisionAt,
        updatedBy: guide.revisionBy,
        headEventId: guide.headEventId ?? "2".repeat(64),
      },
    ],
    revisions: { [GUIDE_ID]: [guide] },
  };
}

function readyViewAtRevision(
  revision: number,
  options?: {
    issueTitle?: string;
    issueObjectRevision?: number;
    issueUpdatedBy?: string;
  },
) {
  const next = structuredClone(V3_READY_VIEW) as Extract<
    RawProjectViewLoadResult,
    { status: "ready" }
  >;
  next.project_revision = revision;
  next.updated_at = `2026-07-27T08:0${revision}:00Z`;
  for (const object of next.objects_v3) {
    object.project_revision = revision;
  }
  for (const role of next.role_continuity.roles) {
    role.project_revision = revision;
  }
  for (const brief of next.role_continuity.briefs) {
    brief.project_revision = revision;
  }
  const nextIssue = next.objects_v3.find(
    (candidate) => candidate.id === IDS.issue,
  );
  if (nextIssue) {
    nextIssue.project_revision = revision;
    nextIssue.object_revision =
      options?.issueObjectRevision ?? nextIssue.object_revision;
    if (options?.issueTitle) nextIssue.data.data.title = options.issueTitle;
    if (options?.issueUpdatedBy) {
      nextIssue.updated_by = options.issueUpdatedBy;
    }
  }
  return next;
}

function emptyRoleContinuity() {
  return {
    roles: [],
    proposals: [],
    assignments: [],
    commitments: [],
    workResponsibilities: [],
    checkpoints: [],
    handoffs: [],
    members: [{ pubkey: HUMAN, role: "owner" as const }],
    briefs: [],
  } satisfies Extract<
    RawProjectViewLoadResult,
    { status: "ready" }
  >["role_continuity"];
}

function minimalReadyView(name: string, revision = 7) {
  const minimalProfile = object(
    "project_profile",
    "10000000-0000-4000-8000-000000000001",
    {
      name,
      positioning: "A separate Community-scoped project.",
      purpose: "Prove Project View isolation.",
      problem: "State must not cross Relay boundaries.",
      scope: "This Community only.",
    },
  );
  const minimalGoal = object("goal", "10000000-0000-4000-8000-000000000002", {
    title: `${name} goal`,
    desired_outcome: "One isolated verified View.",
    directions: [],
  });
  minimalProfile.project_revision = revision;
  minimalGoal.project_revision = revision;
  return {
    status: "ready",
    relay_pubkey: "c".repeat(64),
    schema_version: 3,
    project_revision: revision,
    projection_generation: 3,
    active_object_count: 2,
    updated_at: NOW,
    objects_v3: [minimalProfile, minimalGoal],
    role_continuity: emptyRoleContinuity(),
  } satisfies RawProjectViewLoadResult;
}

function vacantV3View() {
  const next = structuredClone(V3_READY_VIEW) as Extract<
    RawProjectViewLoadResult,
    { status: "ready" }
  >;
  next.relay_pubkey = "c".repeat(64);
  next.role_continuity.proposals = [];
  next.role_continuity.assignments = [];
  next.role_continuity.commitments = [];
  next.role_continuity.workResponsibilities = [];
  next.role_continuity.checkpoints = [];
  next.role_continuity.handoffs = [];
  next.role_continuity.briefs = [];
  next.role_continuity.members = [
    { pubkey: HUMAN, role: "owner" },
    { pubkey: ACTOR, role: "member" },
  ];
  return next;
}

function memberViewerV3View() {
  const next = vacantV3View();
  next.role_continuity.members = [
    { pubkey: "f".repeat(64), role: "owner" },
    { pubkey: HUMAN, role: "member" },
    { pubkey: ACTOR, role: "member" },
  ];
  return next;
}

function activeLeaderV3View() {
  const next = structuredClone(V3_READY_VIEW) as Extract<
    RawProjectViewLoadResult,
    { status: "ready" }
  >;
  const continuity = next.role_continuity;
  continuity.members = [
    { pubkey: "f".repeat(64), role: "owner" },
    { pubkey: HUMAN, role: "admin" },
  ];
  continuity.assignments = [
    {
      assignment_id: ROLE_STATE_IDS.currentAssignment,
      role_id: IDS.role,
      member_pubkey: HUMAN,
      started_at: NOW,
      started_by: HUMAN,
      entity_revision: 1,
      project_revision: 7,
    },
  ];
  continuity.proposals = [];
  continuity.commitments = [];
  continuity.workResponsibilities = [];
  continuity.checkpoints = [];
  continuity.handoffs = [];
  continuity.briefs = [];
  return next;
}

function humanAssignedV3View() {
  const next = structuredClone(V3_READY_VIEW) as Extract<
    RawProjectViewLoadResult,
    { status: "ready" }
  >;
  const continuity = next.role_continuity;
  const assignment = continuity.assignments.find(
    (candidate) => candidate.assignment_id === ROLE_STATE_IDS.currentAssignment,
  );
  const commitment = continuity.commitments.find(
    (candidate) => candidate.commitment_id === ROLE_STATE_IDS.commitment,
  );
  const checkpoint = continuity.checkpoints.find(
    (candidate) => candidate.checkpoint_id === ROLE_STATE_IDS.checkpoint,
  );
  const brief = continuity.briefs[0];
  if (
    !assignment ||
    !commitment ||
    !checkpoint ||
    !brief ||
    brief.state.status !== "assigned"
  ) {
    throw new Error("assigned v3 fixture is incomplete");
  }
  assignment.member_pubkey = HUMAN;
  commitment.member_pubkey = HUMAN;
  commitment.started_by = HUMAN;
  checkpoint.created_by = HUMAN;
  brief.member_pubkey = HUMAN;
  brief.community_role = "owner";
  brief.state.assignment.assignment.member_pubkey = HUMAN;
  const directoryAssignment = brief.role_directory.entries[0]?.assignment;
  if (directoryAssignment?.status === "assigned") {
    directoryAssignment.member_pubkey = HUMAN;
  }
  const briefCommitment =
    brief.responsible_work[0]?.state.status === "committed"
      ? brief.responsible_work[0].state.commitment.commitment
      : undefined;
  if (briefCommitment) {
    briefCommitment.member_pubkey = HUMAN;
    briefCommitment.started_by = HUMAN;
  }
  if (brief.latest_checkpoint) {
    brief.latest_checkpoint.checkpoint.created_by = HUMAN;
  }
  return next;
}

async function seedCommunities(
  page: import("@playwright/test").Page,
  activeId = COMMUNITY_A.id,
) {
  await page.addInitScript(
    ({ active, communities }) => {
      window.localStorage.setItem(
        "buzz-communities",
        JSON.stringify(communities),
      );
      window.localStorage.setItem("buzz-active-community-id", active);
    },
    { active: activeId, communities: [COMMUNITY_A, COMMUNITY_B] },
  );
}

async function openFullProjectView(page: import("@playwright/test").Page) {
  await page.goto("/#/view");
}

function projectViewContextResult(): ProjectContextQueryResult {
  return {
    communityKey: "fixture",
    projectId: "40000000-0000-4000-8000-000000000001",
    relayPubkey: "b".repeat(64),
    context: {
      contextRevision: 1,
      projectionGeneration: 1,
      activeEdgeCount: 0,
      boundDocumentCount: 0,
      updatedAt: NOW,
      metaEventId: "c".repeat(64),
      capabilityEnabled: true,
    },
    query: { type: "contains_all", coordinates: [] },
    projectViewObservation: { state: "observed" },
    documentObservation: { state: "not_requested" },
    edges: [],
    coordinateDetails: [
      {
        coordinateKey: `requirement:${IDS.requirement}`,
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId: IDS.requirement,
        },
        state: "active",
        title: "Verified snapshot",
      },
    ],
    documentDetails: [],
  };
}

test("active Project View object opens Incident Context independent of Context References", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectContext: projectViewContextResult(),
    projectView: V3_READY_VIEW,
  });
  await page.goto(`/#/view?object=${IDS.requirement}`);
  await expect(page.getByTestId("project-view-inspector")).toBeVisible();
  await expect(
    page.getByTestId("project-view-show-in-project-context"),
  ).toBeVisible();
  await page.getByTestId("project-view-show-in-project-context").click();

  await expect(page).toHaveURL(/project-context/);
  await expect(page).toHaveURL(/mode=incident/);
  await expect(page).toHaveURL(
    new RegExp(`coordinates=requirement(%3A|:)${IDS.requirement}`),
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
        coordinate: {
          type: "project_view_object",
          objectType: "requirement",
          objectId: IDS.requirement,
        },
      },
    },
  });

  await page.goBack();
  await expect(page).toHaveURL(
    new RegExp(`/view\\?object=${IDS.requirement}$`),
  );
  await expect(page.getByTestId("project-view-inspector")).toBeVisible();
});

test("Community overview presents Project View and Role context before the full map", async ({
  page,
}) => {
  await installMockBridge(page, {
    managedAgents: [{ pubkey: ACTOR, name: "Context Agent" }],
    projectView: V3_READY_VIEW,
  });
  await page.goto("/");

  await expect(page.getByTestId("open-view")).toHaveCount(0);
  await page.getByTestId("open-community-overview").click();

  await expect(page).toHaveURL(/\/community$/);
  await expect(page.getByTestId("community-project-summary")).toContainText(
    "Lora",
  );
  await expect(page.getByTestId("community-current-focus")).toContainText(
    "Projects naming overlap",
  );
  await expect(page.getByTestId("community-role-summary")).toContainText(
    "Context steward",
  );
  await expect(page.getByTestId("community-role-summary")).toContainText(
    "Context Agent",
  );
  await expect(page.getByTestId("community-needs-attention")).toContainText(
    "Projects naming overlap",
  );
  await expect(page.getByTestId("community-needs-attention")).toBeInViewport();
  await expect(page.getByTestId("community-resources")).toContainText(
    "Buzz repository",
  );
  await expect(
    page
      .getByTestId("community-project-overview")
      .getByText("Verified", { exact: true }),
  ).toHaveCount(1);

  await page
    .getByTestId("community-current-focus")
    .locator(`button[data-object-id="${IDS.issue}"]`)
    .click();
  await expect(page).toHaveURL(new RegExp(`\\/view\\?object=${IDS.issue}$`));
  await expect(page.getByTestId("project-view-inspector")).toContainText(
    "Projects naming overlap",
  );
  await page.getByTestId("return-community-overview").click();
  await expect(page).toHaveURL(/\/community$/);
  await expect(page.getByTestId("community-project-summary")).toContainText(
    "Lora",
  );

  await page.getByTestId("open-full-project-view").click();
  await expect(page).toHaveURL(/\/view$/);
  await expect(page.getByTestId("project-view-map")).toBeVisible();
  await expect(page.getByTestId("return-community-overview")).toContainText(
    "E2E Test",
  );
});

test("Community overview keeps its stable shell when Project View preview is disabled", async ({
  page,
}) => {
  await installMockBridge(
    page,
    { projectView: V3_READY_VIEW },
    { seedPreviewFeatures: false },
  );
  await page.goto("/#/community");

  await expect(page.getByTestId("community-space-header")).toContainText(
    "E2E Test",
  );
  await expect(page.getByTestId("community-continue-work")).toContainText(
    "Open Inbox",
  );
  const disabledState = page.getByTestId("community-project-view-disabled");
  await expect(disabledState).toBeVisible();
  await expect(disabledState).toContainText(
    "Community navigation and your last work position remain available.",
  );
  const disabledBox = await disabledState.boundingBox();
  expect(disabledBox).not.toBeNull();
  expect(disabledBox?.height ?? Number.POSITIVE_INFINITY).toBeLessThan(180);

  const projectViewReads = await page.evaluate(
    () =>
      (window.__BUZZ_E2E_COMMAND_LOG__ ?? []).filter(
        (entry) => entry.command === "get_project_view",
      ).length,
  );
  expect(projectViewReads).toBe(0);
});

test("View renders the verified canonical map and object inspector", async ({
  page,
}) => {
  await installMockBridge(page, {
    managedAgents: [{ pubkey: ACTOR, name: "Context Agent" }],
    projectView: READY_VIEW,
  });
  await page.goto("/");

  await expect(page.getByTestId("open-projects-view")).toContainText(
    "Projects",
  );
  await openFullProjectView(page);

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
  await expect(page.getByTestId("project-view-inspector")).toContainText(
    "Context Agent",
  );
  await expect(page.getByTestId("project-view-inspector")).toContainText(
    "Agent",
  );

  await page.getByRole("button", { name: "Close inspector" }).click();
  await expect(page).toHaveURL(/\/view$/);
});

test("v3 Resource creation uses an active Guide and never exposes a legacy locator", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectDocument: guideDocumentState(),
    projectView: V3_READY_VIEW,
    projectViewMutationResult: {
      status: "applied",
      event_id: "c".repeat(64),
      project_revision: 8,
      object_id: "50000000-0000-4000-8000-000000000001",
      object_revision: 1,
      deleted: false,
    },
  });
  await page.goto("/");
  await openFullProjectView(page);

  await page.getByRole("button", { name: "Add Resource" }).click();
  await expect(page.getByLabel("Locator")).toHaveCount(0);
  await expect(page.getByLabel("Resource type")).toHaveCount(0);
  await page.getByLabel("Name").fill("Deployment service");
  await page.getByLabel("Resource kind").fill("internal-service-v9");
  await page.getByLabel("Summary").fill("Unknown kinds remain first-class.");
  await page.getByLabel("Guide").selectOption(GUIDE_ID);
  await page.getByRole("button", { name: "Create Resource" }).click();

  const mutations = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  );
  expect(mutations?.[0]).toMatchObject({
    operation: "create",
    expected_project_revision: 7,
    object_type: "resource",
    data: {
      name: "Deployment service",
      resource_kind: "internal-service-v9",
      summary: "Unknown kinds remain first-class.",
      guide_document_id: GUIDE_ID,
    },
  });

  await page
    .getByRole("button", { name: "Inspect Resource Buzz repository" })
    .click();
  await page.getByRole("link", { name: "Open verified Guide" }).click();
  await expect(page).toHaveURL(new RegExp(`/documents\\?document=${GUIDE_ID}`));
  await expect(
    page.getByRole("heading", { name: "Buzz repository guide" }),
  ).toBeVisible();
  await expect(page.getByTestId("document-markdown")).toContainText(
    "reviewed clone and contribution workflow",
  );
});

test("v3 Role Context picker submits canonical coordinates and Resource sources hide Resource targets", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectDocument: guideDocumentState(),
    projectView: contextReadyV3View(),
  });
  await page.goto("/");
  await openFullProjectView(page);

  await page
    .getByRole("button", { name: "Inspect Role Context steward" })
    .click();
  const context = page.getByTestId("project-view-context");
  await expect(context).toContainText("Context References");
  await expect(context).toContainText("Verified coordinates only");
  await expect(context).toContainText("Ready");
  await page
    .getByLabel("Context target type", { exact: true })
    .selectOption("resource");
  await page
    .getByLabel("Context target", { exact: true })
    .selectOption(IDS.resource);
  await page.getByRole("button", { name: "Add Context" }).click();

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(1);
  const mutations = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  );
  expect(mutations?.[0]).toEqual({
    operation: "context",
    expected_project_revision: 7,
    object_type: "role",
    object_id: IDS.role,
    context_references: [{ type: "resource", resource_id: IDS.resource }],
  });

  await page.getByRole("button", { name: "Close inspector" }).click();
  await page
    .getByRole("button", { name: "Inspect Resource Buzz repository" })
    .click();
  const targetType = page.getByLabel("Context target type", { exact: true });
  await expect(targetType.locator('option[value="resource"]')).toHaveCount(0);
  await expect(targetType).toHaveValue("live_document");
});

test("disabled Context keeps preserved coordinates visible and permits only cleanup", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: contextUnavailableWithPreservedIssueRef(),
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page
    .getByRole("button", { name: "Inspect Issue Projects naming overlap" })
    .click();

  const context = page.getByTestId("project-view-context");
  await expect(context).toContainText("Unavailable");
  await expect(context).toContainText("Buzz repository");
  await expect(context).toContainText("Preserved coordinates remain visible");
  await expect(page.getByRole("button", { name: "Add Context" })).toHaveCount(
    0,
  );
  await page
    .getByRole("button", { name: "Remove Context Buzz repository" })
    .click();

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(1);
  const mutations = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  );
  expect(mutations?.[0]).toEqual({
    operation: "context",
    expected_project_revision: 7,
    object_type: "issue",
    object_id: IDS.issue,
    context_references: [],
  });
});

test("v3 Resource saga preserves a newly committed Guide when the Resource conflicts", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectDocument: guideDocumentState(),
    projectView: V3_READY_VIEW,
    projectViewMutationResults: [
      {
        status: "conflict",
        expected_project_revision: 7,
        current_project_revision: 8,
        message: "relay returned 409: project revision conflict",
      },
    ],
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page.getByRole("button", { name: "Add Resource" }).click();

  await page.getByLabel("Name").fill("Canary environment");
  await page.getByLabel("Resource kind").fill("environment");
  await page.getByLabel("Guide").selectOption({ label: "Create a new Guide…" });
  await page.getByLabel("Guide title").fill("Canary environment guide");
  await page.getByLabel("Guide summary").fill("Bounded canary instructions");
  await page
    .getByLabel("Guide Markdown")
    .fill("# Canary\n\nRun only in the declared cohort.");
  await page.getByRole("button", { name: "Create Resource" }).click();

  await expect(page.getByRole("alert")).toContainText("Project changed");
  await expect(
    page.getByText(
      "Guide created and preserved; review the latest View, then retry the Resource.",
    ),
  ).toBeVisible();
  const submitted = await page.evaluate(() => ({
    documents: window.__BUZZ_E2E_PROJECT_DOCUMENT_CALLS__?.filter(
      (call) => call.command === "mutate_project_document",
    ),
    projectView: window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  }));
  expect(submitted.documents).toHaveLength(1);
  expect(submitted.documents?.[0]).toMatchObject({
    payload: {
      input: {
        mutation: {
          type: "create",
          title: "Canary environment guide",
          summary: "Bounded canary instructions",
          contentMarkdown: "# Canary\n\nRun only in the declared cohort.",
        },
      },
    },
  });
  expect(submitted.projectView).toHaveLength(1);
  expect(submitted.projectView?.[0]).toMatchObject({
    operation: "create",
    expected_project_revision: 7,
    object_type: "resource",
    data: {
      name: "Canary environment",
      resource_kind: "environment",
    },
  });
  const guideId = (
    submitted.projectView?.[0] as {
      data?: { guide_document_id?: string };
    }
  )?.data?.guide_document_id;
  expect(guideId).toMatch(
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
  );
  await expect(page.getByLabel("Guide")).toHaveValue(guideId ?? "");
});

test("v3 Role cards and Inspector show one verified continuity state", async ({
  page,
}) => {
  await installMockBridge(page, {
    managedAgents: [{ pubkey: ACTOR, name: "Context Agent" }],
    projectView: V3_READY_VIEW,
  });
  await page.goto("/");
  await openFullProjectView(page);

  const roleCard = page.getByTestId(`project-role-card-${IDS.role}`);
  await expect(roleCard).toContainText("Leader");
  await expect(roleCard).toContainText("Assigned");
  await expect(roleCard).toContainText("Context Agent");
  await roleCard.click();

  const inspector = page.getByTestId("project-view-inspector");
  await expect(inspector).toContainText("Leader · admin");
  await expect(inspector).toContainText("Current tenure");
  await expect(page.getByTestId("project-role-brief")).toContainText(
    "Verified Role Brief",
  );
  await expect(page.getByTestId("project-role-brief")).toContainText(
    "Make project context legible",
  );
  await expect(page.getByTestId("project-role-directory")).toContainText(
    "Collaboration roles",
  );
  await expect(page.getByTestId("project-role-directory")).toContainText(
    "Context steward",
  );
  await expect(page.getByTestId("project-role-directory")).toContainText(
    "Current",
  );
  await expect(page.getByTestId("project-role-brief")).toContainText(
    "Add the View entry",
  );
  await expect(page.getByTestId("project-role-brief")).toContainText(
    "Committed",
  );
  await expect(inspector).toContainText(
    "Return to project context stewardship",
  );
  await expect(inspector).toContainText("Tenure history");
  await expect(inspector).toContainText("Continuity timeline");
  await expect(
    page.getByTestId("project-role-latest-checkpoint"),
  ).toContainText("The View is usable; finish the continuity timeline.");
  await expect(page.getByTestId("project-role-timeline")).toContainText(
    "Continue from the verified Project View.",
  );
  await expect(
    inspector.getByRole("button", { name: "Delete" }),
  ).toBeDisabled();
  await expect(page.getByTestId("project-role-lifecycle-guard")).toBeVisible();

  await inspector.getByRole("button", { name: "Edit" }).click();
  await expect(page.getByLabel("Active role")).toBeDisabled();
});

test("ordinary member can inspect and request Roles but cannot govern them", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: memberViewerV3View() });
  await page.goto("/");
  await openFullProjectView(page);

  await expect(page.getByRole("button", { name: "Add Role" })).toHaveCount(0);
  await page.getByTestId("project-view-add").click();
  await expect(
    page.getByLabel("Object type").locator('option[value="role"]'),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Cancel" }).click();

  await page.getByTestId(`project-role-card-${IDS.role}`).click();
  const inspector = page.getByTestId("project-view-inspector");
  await expect(inspector.getByRole("button", { name: "Edit" })).toHaveCount(0);
  await expect(inspector.getByRole("button", { name: "Delete" })).toHaveCount(
    0,
  );
  await expect(page.getByTestId("project-role-request")).toBeVisible();
});

test("owner creates an admin Role with an explicit signed level", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: V3_READY_VIEW });
  await page.goto("/");
  await openFullProjectView(page);

  await page.getByRole("button", { name: "Add Role" }).click();
  await page.getByLabel("Role level").selectOption("admin");
  await page.getByLabel("Name").fill("Delivery leader");
  await page.getByLabel("Purpose").fill("Govern member Roles");
  await page.getByRole("button", { name: "Create Role" }).click();

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(1);
  const intent = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?.[0],
  );
  expect(intent).toMatchObject({
    operation: "create",
    object_type: "role",
    initial_role_level: "admin",
  });
  expect(intent).not.toHaveProperty("acting_assignment_id");
});

test("active Leader creates only member Roles through the exact Assignment", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: activeLeaderV3View(),
  });
  await page.goto("/");
  await openFullProjectView(page);

  await page.getByRole("button", { name: "Add Role" }).click();
  await expect(page.getByLabel("Role level")).toHaveValue("member");
  await expect(
    page.getByLabel("Role level").locator('option[value="admin"]'),
  ).toHaveCount(0);
  await page.getByLabel("Name").fill("Release steward");
  await page.getByLabel("Purpose").fill("Coordinate releases");
  await page.getByRole("button", { name: "Create Role" }).click();

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(1);
  const intent = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?.[0],
  );
  expect(intent).toMatchObject({
    operation: "create",
    object_type: "role",
    initial_role_level: "member",
    acting_assignment_id: ROLE_STATE_IDS.currentAssignment,
  });
});

test("an open Role Proposal blocks deletion and deactivation", async ({
  page,
}) => {
  const projectView = structuredClone(V3_READY_VIEW);
  projectView.role_continuity.assignments = [];
  projectView.role_continuity.commitments = [];
  projectView.role_continuity.workResponsibilities = [];
  projectView.role_continuity.briefs = [];
  await installMockBridge(page, { projectView });
  await page.goto("/");
  await openFullProjectView(page);

  await page.getByTestId(`project-role-card-${IDS.role}`).click();
  const inspector = page.getByTestId("project-view-inspector");
  await expect(
    inspector.getByRole("button", { name: "Delete" }),
  ).toBeDisabled();
  await expect(page.getByTestId("project-role-lifecycle-guard")).toContainText(
    "This Role has an open Proposal",
  );

  await inspector.getByRole("button", { name: "Edit" }).click();
  await expect(page.getByLabel("Active role")).toBeDisabled();
  await expect(
    page.getByText(
      "Resolve or withdraw the open Proposal before deactivating this Role.",
    ),
  ).toBeVisible();
});

test("Role Inspector loads the next history page through the native boundary", async ({
  page,
}) => {
  const cursorId = "00000000-0000-4000-8000-000000000091";
  const pages: RawProjectRoleHistoryPage[] = [
    {
      project_revision: 7,
      projection_generation: 2,
      items: [
        {
          entity_type: "checkpoint",
          entity: {
            checkpoint_id: cursorId,
            role_id: IDS.role,
            assignment_id: ROLE_STATE_IDS.formerAssignment,
            based_on_project_revision: 4,
            content: {
              summary: "First bounded history page.",
              current_focus: [],
              progress: [],
              blockers: [],
              risks: [],
              open_questions: [],
              next_steps: [],
              references: [],
            },
            created_by: FORMER_ASSIGNEE,
            created_at: "2026-07-25T08:00:00Z",
            entity_revision: 1,
            project_revision: 5,
          },
        },
      ],
      next_before: {
        project_revision: 5,
        entity_type: "role_checkpoint",
        entity_id: cursorId,
      },
    },
    {
      project_revision: 7,
      projection_generation: 2,
      items: [
        {
          entity_type: "handoff",
          entity: {
            handoff_id: "00000000-0000-4000-8000-000000000092",
            role_id: IDS.role,
            from_assignment_id: ROLE_STATE_IDS.formerAssignment,
            affected_commitment_ids: [],
            content: {
              summary: "Loaded from the server-side continuation page.",
              unresolved_items: [],
              references: [],
            },
            cause: "planned",
            system_generated: false,
            created_by: FORMER_ASSIGNEE,
            created_at: "2026-07-24T08:00:00Z",
            entity_revision: 1,
            project_revision: 4,
          },
        },
      ],
    },
  ];
  await installMockBridge(page, {
    projectView: V3_READY_VIEW,
    projectViewRoleHistoryPages: pages,
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page.getByTestId(`project-role-card-${IDS.role}`).click();

  await expect(page.getByText("First bounded history page.")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          window.__BUZZ_E2E_PROJECT_VIEW_ROLE_HISTORY_REQUESTS__?.length ?? 0,
      ),
    )
    .toBe(1);
  await page.getByTestId("project-role-timeline-more").click();
  await expect(
    page.getByText("Loaded from the server-side continuation page."),
  ).toBeVisible();
  const requests = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_HISTORY_REQUESTS__,
  );
  expect(requests).toHaveLength(2);
  expect(requests?.[1]).toMatchObject({
    project_revision: 7,
    projection_generation: 3,
    role_id: IDS.role,
    before: {
      project_revision: 5,
      entity_type: "role_checkpoint",
      entity_id: cursorId,
    },
  });
});

test("Work Inspector shows the verified responsibility and Commitment", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: V3_READY_VIEW,
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page
    .getByRole("button", { name: "Inspect Work Add the View entry" })
    .click();

  const continuity = page.getByTestId("project-work-continuity");
  await expect(continuity).toContainText("Context steward");
  await expect(continuity).toContainText("Committed");
  await expect(page.getByLabel("Responsible Role")).toBeDisabled();
});

test("owner assigns uncommitted Work to a Role with a revision fence", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: vacantV3View(),
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page
    .getByRole("button", { name: "Inspect Work Add the View entry" })
    .click();
  await page.getByLabel("Responsible Role").selectOption(IDS.role);
  await page.getByRole("button", { name: "Save responsibility" }).click();

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(1);
  const intent = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.[0],
  );
  expect(intent).toMatchObject({
    operation: "set_work_responsibility",
    expected_project_revision: 7,
    work_id: IDS.work,
    responsible_role_id: IDS.role,
  });
});

test("owner creates a revision-fenced Role offer from the Inspector", async ({
  page,
}) => {
  await installMockBridge(page, {
    managedAgents: [
      {
        pubkey: ROLE_CANDIDATE,
        name: "test-2",
        relayUrl: "",
        status: "stopped",
      },
    ],
    managedAgentRuntimes: [
      {
        pubkey: ROLE_CANDIDATE,
        relayUrl: "ws://127.0.0.1:3000",
        lifecycle: "ready",
      },
    ],
    projectView: V3_READY_VIEW,
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page.getByTestId(`project-role-card-${IDS.role}`).click();
  await page.getByTestId("project-role-offer").click();
  await page.getByTestId("project-role-candidate-picker").click();
  await page.getByTestId("project-role-candidate-search").fill("test-2");
  await expect(
    page.getByTestId(`project-role-candidate-option-${ROLE_CANDIDATE}`),
  ).toContainText("managed by you");
  await expect(
    page.getByTestId(`project-role-candidate-option-${ROLE_CANDIDATE}`),
  ).toContainText("Running");
  await page
    .getByTestId(`project-role-candidate-option-${ROLE_CANDIDATE}`)
    .click();
  await expect(page.getByTestId("project-role-candidate-picker")).toContainText(
    "test-2",
  );
  await page.getByTestId("project-role-offer-submit").click();

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(1);
  const intent = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.[0],
  );
  expect(intent).toMatchObject({
    operation: "offer_role",
    expected_project_revision: 7,
    role_id: IDS.role,
    candidate_pubkey: ROLE_CANDIDATE,
    expires_in_hours: 72,
  });
});

test("the current assignee appends Checkpoint and Handoff context", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: humanAssignedV3View(),
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page.getByTestId(`project-role-card-${IDS.role}`).click();
  await page.getByTestId("project-role-checkpoint").click();
  await page
    .getByLabel("Situation summary")
    .fill("The naming decision is ready for implementation.");
  await page
    .getByLabel("Current focus")
    .fill("Rename the navigation entry\nPreserve old routes");
  await page
    .getByLabel("Blockers")
    .fill("Awaiting final copy review\nDesktop snapshot is stale");
  await page.getByTestId("project-role-checkpoint-submit").click();

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(1);
  const intent = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.[0],
  );
  expect(intent).toMatchObject({
    operation: "append_checkpoint",
    expected_project_revision: 7,
    based_on_project_revision: 7,
    acting_assignment_id: ROLE_STATE_IDS.currentAssignment,
    content: {
      summary: "The naming decision is ready for implementation.",
      current_focus: ["Rename the navigation entry", "Preserve old routes"],
      blockers: ["Awaiting final copy review", "Desktop snapshot is stale"],
      references: [],
    },
  });

  await page.getByTestId("project-role-handoff").click();
  await page
    .getByLabel("Transition summary")
    .fill("Keep the route migration reversible.");
  await page
    .getByLabel("Unresolved items")
    .fill("Confirm final navigation copy\nCapture a fresh screenshot");
  await page.getByTestId("project-role-handoff-submit").click();
  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(2);
  const handoffIntent = await page.evaluate(
    () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.[1],
  );
  expect(handoffIntent).toMatchObject({
    operation: "append_handoff",
    expected_project_revision: 7,
    acting_assignment_id: ROLE_STATE_IDS.currentAssignment,
    checkpoint_id: ROLE_STATE_IDS.checkpoint,
    cause: "planned",
    content: {
      summary: "Keep the route migration reversible.",
      unresolved_items: [
        "Confirm final navigation copy",
        "Capture a fresh screenshot",
      ],
      references: [],
    },
  });
});

test("a concurrent Role replacement refreshes state without replaying intent", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: V3_READY_VIEW,
    projectViewRoleMutationResult: {
      status: "conflict",
      expected_project_revision: 7,
      current_project_revision: 8,
      message: "conflict:project_view_v3:revision_conflict",
    },
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page.getByTestId(`project-role-card-${IDS.role}`).click();
  await page.getByTestId("project-role-offer").click();
  await page.getByTestId("project-role-candidate-picker").click();
  await page.getByTestId("project-role-candidate-manual-toggle").click();
  await page.getByTestId("project-role-candidate").fill(FORMER_ASSIGNEE);
  await page.getByTestId("project-role-offer-submit").click();

  await expect(
    page.getByText(/changed before this Role action was applied/),
  ).toBeVisible();
  await page.waitForTimeout(250);
  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_PROJECT_VIEW_ROLE_MUTATIONS__?.length ?? 0,
      ),
    )
    .toBe(1);
});

test("v3 Community settings route Role changes through View", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: V3_READY_VIEW,
    relayRequiresMembership: true,
  });
  await page.goto("/");
  await openSettings(page, "community-members");

  await expect(
    page.getByTestId("community-members-manage-in-view"),
  ).toBeVisible();
  await expect(page.getByTestId("community-invite-dialog-trigger")).toHaveCount(
    0,
  );
  await page.getByTestId(`relay-member-actions-${ALICE}`).click();
  await expect(
    page.getByRole("menuitem", { name: "Manage Role in View" }),
  ).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Make member" })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("menuitem", { name: "Remove from community" }),
  ).toHaveCount(0);

  await page.getByRole("menuitem", { name: "Manage Role in View" }).click();
  await expect(page).toHaveURL(/\/view$/);
});

test("View keeps a stable skeleton until the first snapshot is verified", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: READY_VIEW,
    projectViewReadDelayMs: 500,
  });
  await page.goto("/");
  await openFullProjectView(page);

  await expect(page.getByTestId("project-view-loading-skeleton")).toBeVisible();
  await expect(page.getByTestId("project-view-profile")).toHaveCount(0);
  await expect(page.getByTestId("project-view-profile")).toContainText("Lora", {
    timeout: 5_000,
  });
});

test("View rejects a self-contradictory snapshot without rendering partial data", async ({
  page,
}) => {
  const invalid = structuredClone(READY_VIEW) as Extract<
    RawProjectViewLoadResult,
    { status: "ready" }
  >;
  invalid.active_object_count = 11;
  await installMockBridge(page, { projectView: invalid });
  await page.goto("/");
  await openFullProjectView(page);

  await expect(
    page.getByRole("heading", { name: "View integrity check failed" }),
  ).toBeVisible();
  await expect(page.getByTestId("project-view-profile")).toHaveCount(0);
  await page.getByText("Diagnostic detail").click();
  await expect(page.getByText(/active object count 11/)).toBeVisible();
});

test("View rejects a Role Directory that disagrees with verified continuity", async ({
  page,
}) => {
  const invalid = structuredClone(V3_READY_VIEW) as Extract<
    RawProjectViewLoadResult,
    { status: "ready" }
  >;
  const directoryAssignment =
    invalid.role_continuity?.briefs[0]?.role_directory.entries[0]?.assignment;
  if (directoryAssignment?.status !== "assigned") {
    throw new Error("v3 fixture must include an assigned Role Directory entry");
  }
  directoryAssignment.member_pubkey = FORMER_ASSIGNEE;

  await installMockBridge(page, { projectView: invalid });
  await page.goto("/");
  await openFullProjectView(page);

  await expect(
    page.getByRole("heading", { name: "View integrity check failed" }),
  ).toBeVisible();
  await expect(page.getByTestId("project-view-profile")).toHaveCount(0);
  await page.getByText("Diagnostic detail").click();
  await expect(
    page.getByText(/Role Brief Role Directory disagrees with Role continuity/),
  ).toBeVisible();
});

test("View explains a trusted-read failure without rendering project data", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectViewReadError: "Relay snapshot verification timed out",
  });
  await page.goto("/");
  await openFullProjectView(page);

  await expect(
    page.getByRole("heading", { name: "View could not be verified" }),
  ).toBeVisible();
  await expect(
    page.getByText("Relay snapshot verification timed out"),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expect(page.getByTestId("project-view-profile")).toHaveCount(0);
});

test("an intentionally sparse View explains every major empty section", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: minimalReadyView("Sparse") });
  await page.goto("/");
  await openFullProjectView(page);

  await expect(page.getByTestId("project-view-profile")).toContainText(
    "Sparse",
  );
  await expect(page.getByText("This goal has no bound plan.")).toBeVisible();
  await expect(page.getByText("No semantic roles declared.")).toBeVisible();
  await expect(page.getByText("No resources declared.")).toBeVisible();
});

test("arrow keys traverse the project map and Escape restores card focus", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: READY_VIEW });
  await page.goto("/");
  await openFullProjectView(page);

  const goalCard = page.locator(`button[data-object-id="${IDS.goal}"]`);
  const planCard = page.locator(`button[data-object-id="${IDS.plan}"]`);
  const lastCard = page.locator(`button[data-object-id="${IDS.looseIssue}"]`);
  await goalCard.focus();
  await page.keyboard.press("ArrowDown");
  await expect(planCard).toBeFocused();
  await page.keyboard.press("End");
  await expect(lastCard).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("project-view-inspector")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("project-view-inspector")).toHaveCount(0);
  await expect(lastCard).toBeFocused();
});

test("the Inspector becomes a focus-trapped drawer in a narrow window", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: READY_VIEW });
  await page.goto("/");
  await openFullProjectView(page);
  await page.setViewportSize({ width: 560, height: 720 });

  const issueCard = page.locator(`button[data-object-id="${IDS.issue}"]`);
  await issueCard.click();
  const inspector = page.getByTestId("project-view-inspector");
  await expect(inspector).toHaveAttribute("data-presentation", "drawer");
  await expect(inspector).toHaveAttribute("role", "dialog");
  await page.keyboard.press("Escape");
  await expect(inspector).toHaveCount(0);
  await expect(issueCard).toBeFocused();
});

test("an uninitialized v3 View exposes only the operator and owner setup guide", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: {
      status: "uninitialized",
      relay_pubkey: "b".repeat(64),
    },
  });
  await page.goto("/");
  await openFullProjectView(page);

  await expect(
    page.getByRole("heading", {
      name: "Project View v3 requires owner initialization",
    }),
  ).toBeVisible();
  await expect(
    page.getByText(
      /Desktop did not find an initialized canonical Project View/,
    ),
  ).toBeVisible();
  await expect(
    page.getByText(/buzz-admin project-view prepare-v3/),
  ).toBeVisible();
  await expect(page.getByText(/buzz project-view init-v3/)).toBeVisible();
  await expect(page.getByLabel("Project name")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Initialize View" }),
  ).toHaveCount(0);
  const mutations = await page.evaluate(
    () =>
      (
        window as typeof window & {
          __BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?: Array<Record<string, unknown>>;
        }
      ).__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  );
  expect(mutations ?? []).toHaveLength(0);
});

test("context creation preselects only a legal parent relation", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: READY_VIEW,
    projectViewMutationResult: {
      status: "applied",
      event_id: "c".repeat(64),
      project_revision: 8,
      object_id: "00000000-0000-4000-8000-000000000099",
      object_revision: 1,
      deleted: false,
    },
  });
  await page.goto("/");
  await openFullProjectView(page);

  await page.getByRole("button", { name: "Add Stage" }).first().click();
  await expect(
    page.getByRole("heading", { name: "Add to View" }),
  ).toBeVisible();
  await expect(page.getByLabel("Parent Plan")).toHaveValue(IDS.plan);
  await page.getByLabel("Title").fill("Human editing");
  await page
    .getByLabel("Description")
    .fill("Expose typed Project View mutations.");
  await page.getByRole("button", { name: "Create Stage" }).click();

  const mutations = await page.evaluate(
    () =>
      (
        window as typeof window & {
          __BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?: Array<Record<string, unknown>>;
        }
      ).__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  );
  expect(mutations?.[0]).toMatchObject({
    operation: "create",
    expected_project_revision: 7,
    object_type: "stage",
    data: {
      title: "Human editing",
      status: "planned",
      under_plan_id: IDS.plan,
    },
  });
});

test("a stale edit preserves its draft and requires an explicit new base", async ({
  page,
}) => {
  const revisionEight = readyViewAtRevision(8, {
    issueObjectRevision: 2,
    issueTitle: "Agent changed the naming issue",
  });
  await installMockBridge(page, {
    projectView: READY_VIEW,
    projectViewMutationResults: [
      {
        status: "conflict",
        expected_project_revision: 7,
        current_project_revision: 8,
        message: "relay returned 409: project revision conflict",
      },
      {
        status: "applied",
        event_id: "c".repeat(64),
        project_revision: 9,
        object_id: IDS.issue,
        object_revision: 3,
        deleted: false,
      },
    ],
    projectViewAfterMutation: readyViewAtRevision(9),
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page
    .getByRole("button", { name: "Inspect Issue Projects naming overlap" })
    .click();

  await page.getByRole("button", { name: "Edit" }).click();
  const title = page.getByLabel("Title");
  await title.fill("Projects naming conflict");
  await page.evaluate((next) => {
    window.__BUZZ_E2E_SET_PROJECT_VIEW__?.(next);
  }, revisionEight);
  await page.getByRole("button", { name: "Save changes" }).click();

  await expect(page.getByRole("alert")).toContainText("Project changed");
  await expect(title).toHaveValue("Projects naming conflict");
  await expect(page.getByRole("alert")).toContainText(
    "Latest verified snapshot: revision 8",
  );
  await expect(page.getByRole("alert")).toContainText(
    "changed from object revision 1 to 2",
  );
  let mutations = await page.evaluate(
    () =>
      (
        window as typeof window & {
          __BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?: unknown[];
        }
      ).__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  );
  expect(mutations).toHaveLength(1);

  await page.getByRole("button", { name: "Use revision 8 as base" }).click();
  await expect(title).toHaveValue("Projects naming conflict");
  await expect(
    page.getByText(/Draft now uses verified project revision 8/),
  ).toBeVisible();
  await page.getByRole("button", { name: "Save changes" }).click();

  mutations = await page.evaluate(
    () =>
      (
        window as typeof window & {
          __BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?: Array<Record<string, unknown>>;
        }
      ).__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  );
  expect(mutations).toHaveLength(2);
  expect(mutations?.[0]).toMatchObject({ expected_project_revision: 7 });
  expect(mutations?.[1]).toMatchObject({ expected_project_revision: 8 });
});

test("projection events invalidate into a new complete verified snapshot", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: READY_VIEW });
  await page.goto("/");
  await openFullProjectView(page);
  await expect(page.getByText("Project revision 7")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_HAS_PROJECT_VIEW_SUBSCRIPTION__?.() ?? false,
      ),
    )
    .toBe(true);

  const revisionEight = readyViewAtRevision(8, {
    issueTitle: "Agent refreshed this issue",
    issueObjectRevision: 2,
  });
  await page.evaluate(
    ({ next, kind, tag }) => {
      window.__BUZZ_E2E_SET_PROJECT_VIEW__?.(next);
      window.__BUZZ_E2E_EMIT_PROJECT_VIEW_EVENT__?.({
        kind,
        tag,
      });
    },
    {
      next: revisionEight,
      kind: KIND_PROJECT_VIEW_OBJECT,
      tag: PROJECT_VIEW_V3_OBJECT_TAG,
    },
  );

  await expect(page.getByText("Project revision 8")).toBeVisible();
  await expect(page.getByTestId("project-view-map")).toContainText(
    "Agent refreshed this issue",
  );
});

test("Human and Agent changes alternate through one trusted View", async ({
  page,
}) => {
  const humanRevision = readyViewAtRevision(8, {
    issueObjectRevision: 2,
    issueTitle: "Human clarified the naming issue",
    issueUpdatedBy: HUMAN,
  });
  await installMockBridge(page, {
    managedAgents: [{ pubkey: ACTOR, name: "Context Agent" }],
    projectView: READY_VIEW,
    projectViewAfterMutation: humanRevision,
    projectViewMutationResult: {
      status: "applied",
      event_id: "c".repeat(64),
      project_revision: 8,
      object_id: IDS.issue,
      object_revision: 2,
      deleted: false,
    },
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page
    .getByRole("button", { name: "Inspect Issue Projects naming overlap" })
    .click();
  await page.getByRole("button", { name: "Edit" }).click();
  await page.getByLabel("Title").fill("Human clarified the naming issue");
  await page.getByRole("button", { name: "Save changes" }).click();

  await expect(page.getByText("Project revision 8")).toBeVisible();
  await expect(page.getByTestId("project-view-inspector")).toContainText(
    "Human clarified the naming issue",
  );
  await expect(page.getByTestId("project-view-inspector")).toContainText("You");

  const agentRevision = readyViewAtRevision(9, {
    issueObjectRevision: 3,
    issueTitle: "Agent incorporated the Human decision",
    issueUpdatedBy: ACTOR,
  });
  await page.evaluate(
    ({ next, kind, tag }) => {
      window.__BUZZ_E2E_SET_PROJECT_VIEW__?.(next);
      window.__BUZZ_E2E_EMIT_PROJECT_VIEW_EVENT__?.({
        kind,
        tag,
      });
    },
    {
      next: agentRevision,
      kind: KIND_PROJECT_VIEW_OBJECT,
      tag: PROJECT_VIEW_V3_ENTITY_TAG,
    },
  );

  await expect(page.getByText("Project revision 9")).toBeVisible();
  await expect(page.getByTestId("project-view-inspector")).toContainText(
    "Agent incorporated the Human decision",
  );
  await expect(page.getByTestId("project-view-inspector")).toContainText(
    "Context Agent",
  );
});

test("the v3 setup guide stays read-only and refreshes after owner initialization", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: {
      status: "uninitialized",
      relay_pubkey: "b".repeat(64),
    },
  });
  await page.goto("/");
  await openFullProjectView(page);
  await expect(page.getByTestId("project-view-v3-setup-guide")).toBeVisible();

  await expect
    .poll(() =>
      page.evaluate(
        () => window.__BUZZ_E2E_HAS_PROJECT_VIEW_SUBSCRIPTION__?.() ?? false,
      ),
    )
    .toBe(false);
  await page.evaluate((next) => {
    window.__BUZZ_E2E_SET_PROJECT_VIEW__?.(next);
  }, readyViewAtRevision(8));
  await page
    .getByRole("button", { name: "Check for initialized View" })
    .click();

  await expect(page.getByTestId("project-view-v3-setup-guide")).toHaveCount(0);
  await expect(page.getByTestId("project-view-profile")).toContainText("Lora");
});

test("Community switching does not carry View data or selection across Relays", async ({
  page,
}) => {
  await installMockBridge(
    page,
    {
      projectViewsByRelayUrl: {
        [COMMUNITY_A.relayUrl]: {
          status: "uninitialized",
          relay_pubkey: "b".repeat(64),
        },
        [COMMUNITY_B.relayUrl]: minimalReadyView("Bravo project", 12),
      },
    },
    { skipCommunitySeed: true },
  );
  await seedCommunities(page);
  await page.goto("/");
  await openFullProjectView(page);
  await expect(page.getByTestId("project-view-v3-setup-guide")).toBeVisible();

  await page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`).click();
  await expect(
    page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`),
  ).toHaveAttribute("aria-current", "true");
  await openFullProjectView(page);
  await expect(page.getByTestId("project-view-profile")).toContainText(
    "Bravo project",
  );
  await expect(page.getByTestId("project-view-inspector")).toHaveCount(0);
  await expect(page).not.toHaveURL(/object=/);
  await page
    .getByRole("button", { name: "Inspect Project Profile Bravo project" })
    .click();
  await expect(page.getByTestId("project-view-inspector")).toBeVisible();

  await page.getByTestId(`community-rail-button-${COMMUNITY_A.id}`).click();
  await expect(
    page.getByTestId(`community-rail-button-${COMMUNITY_A.id}`),
  ).toHaveAttribute("aria-current", "true");
  await openFullProjectView(page);
  await expect(
    page.getByRole("heading", {
      name: "Project View v3 requires owner initialization",
    }),
  ).toBeVisible();
  await expect(page.getByTestId("project-view-inspector")).toHaveCount(0);
  await expect(page).not.toHaveURL(/object=/);
});

test("Community switching does not carry an Assignment into another View", async ({
  page,
}) => {
  await installMockBridge(
    page,
    {
      managedAgents: [{ pubkey: ACTOR, name: "Context Agent" }],
      projectViewsByRelayUrl: {
        [COMMUNITY_A.relayUrl]: structuredClone(V3_READY_VIEW),
        [COMMUNITY_B.relayUrl]: vacantV3View(),
      },
    },
    { skipCommunitySeed: true },
  );
  await seedCommunities(page);
  await page.goto("/");
  await openFullProjectView(page);
  await expect(page.getByTestId(`project-role-card-${IDS.role}`)).toContainText(
    "Assigned",
  );

  await page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`).click();
  await expect(
    page.getByTestId(`community-rail-button-${COMMUNITY_B.id}`),
  ).toHaveAttribute("aria-current", "true");
  await openFullProjectView(page);
  const roleCard = page.getByTestId(`project-role-card-${IDS.role}`);
  await expect(roleCard).toContainText("Vacant");
  await expect(roleCard).not.toContainText("Context Agent");
});

test("a disconnected View keeps its verified snapshot and marks it stale", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: READY_VIEW });
  await page.goto("/");
  await openFullProjectView(page);
  await expect(page.getByText("Project revision 7")).toBeVisible();

  await page.evaluate(() => {
    window.__BUZZ_E2E_SET_RELAY_CONNECTION_STATE__?.("disconnected");
  });

  await expect(page.getByText("Offline · may be stale")).toBeVisible();
  await expect(page.getByTestId("project-view-sync-state")).toContainText(
    "Showing verified project revision 7",
  );
  await expect(page.getByTestId("project-view-profile")).toContainText("Lora");
});

test("delete is blocked while an active object still references the target", async ({
  page,
}) => {
  await installMockBridge(page, { projectView: READY_VIEW });
  await page.goto("/");
  await openFullProjectView(page);
  await page
    .getByRole("button", { name: "Inspect Plan Deliver Project View" })
    .click();

  await page.getByRole("button", { name: "Delete" }).click();
  await expect(
    page.getByText("Move or unlink these references first"),
  ).toBeVisible();
  await expect(
    page.getByText(/Stage “Read-only client” references this object/),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Delete object" }),
  ).toBeDisabled();
});

test("an unreferenced object requires confirmation before deletion", async ({
  page,
}) => {
  await installMockBridge(page, {
    projectView: READY_VIEW,
    projectViewMutationResult: {
      status: "applied",
      event_id: "c".repeat(64),
      project_revision: 8,
      object_id: IDS.resource,
      object_revision: 2,
      deleted: true,
    },
  });
  await page.goto("/");
  await openFullProjectView(page);
  await page
    .getByRole("button", { name: "Inspect Resource Buzz repository" })
    .click();

  await page.getByRole("button", { name: "Delete" }).click();
  await expect(
    page.getByRole("heading", { name: "Delete Buzz repository?" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Delete object" }).click();

  const mutations = await page.evaluate(
    () =>
      (
        window as typeof window & {
          __BUZZ_E2E_PROJECT_VIEW_MUTATIONS__?: Array<Record<string, unknown>>;
        }
      ).__BUZZ_E2E_PROJECT_VIEW_MUTATIONS__,
  );
  expect(mutations?.[0]).toMatchObject({
    operation: "delete",
    expected_project_revision: 7,
    object_type: "resource",
    object_id: IDS.resource,
  });
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
    await openFullProjectView(page);

    await expect(
      page.getByRole("heading", { name: state.heading }),
    ).toBeVisible();
  });
}
