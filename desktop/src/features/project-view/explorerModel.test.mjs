import assert from "node:assert/strict";
import test from "node:test";

import {
  buildProjectViewExplorerModel,
  buildProjectViewExplorerPage,
  canonicalObjectOccurrenceKey,
  documentContextOccurrenceKey,
  indexProjectDocumentCatalog,
  issueAboutOccurrenceKey,
  resolveProjectViewExplorerSelection,
  resourceGuideOccurrenceKey,
} from "./explorerModel.ts";
import { projectViewObjectSummary } from "./model.ts";
import { assembleProjectViewV3 } from "../../shared/api/tauriProjectView.ts";

const actor = "a".repeat(64);
const now = "2026-08-15T08:00:00Z";

function objectV3(
  objectType,
  id,
  data,
  relations = {},
  contextReferences = [],
) {
  return {
    id,
    object_type: objectType,
    object_revision: 1,
    project_revision: 9,
    created_at: now,
    updated_at: now,
    created_by: actor,
    updated_by: actor,
    data: { object_type: objectType, data },
    relations,
    context_references: contextReferences,
  };
}

function document(documentId, title, summary) {
  return {
    documentId,
    title,
    summary,
    documentRevision: 8,
    updatedAt: now,
    updatedBy: actor,
    headEventId: `head-${documentId}`,
  };
}

function fixture() {
  const view = assembleProjectViewV3([
    objectV3(
      "project_profile",
      "profile",
      {
        name: "Carryforth",
        summary: "One verified project",
        positioning: "Shared context",
        purpose: "Coordinate delivery",
        problem: "Fragmented state",
        scope: "Project execution",
      },
      {},
      [{ type: "resource", resource_id: "resource" }],
    ),
    objectV3("goal", "goal", {
      title: "Ship the explorer",
      summary: "Deliver focused navigation",
      desired_outcome: "A calm Project View",
      directions: ["Keep one layer visible"],
    }),
    objectV3(
      "plan",
      "plan",
      {
        title: "Desktop delivery",
        summary: "Implement the approved design",
        description: "This full description must not become a summary",
        status: "active",
      },
      { under_goal_id: "goal" },
      [
        { type: "resource", resource_id: "resource" },
        { type: "document", document_id: "doc-live", mode: "live" },
        {
          type: "document",
          document_id: "doc-pinned",
          mode: "pinned",
          document_revision: 3,
        },
      ],
    ),
    objectV3(
      "stage",
      "stage",
      {
        title: "Explorer UI",
        summary: "Build the focused surface",
        description: "Render all explorer UI details",
        status: "active",
      },
      { under_plan_id: "plan" },
    ),
    objectV3(
      "requirement",
      "requirement",
      {
        title: "Single layer",
        summary: "Never render grandchildren",
        description: "Only direct children are visible",
        status: "in_progress",
        priority: "high",
      },
      { planned_in_stage_id: "stage" },
    ),
    objectV3(
      "issue",
      "issue-stage",
      {
        title: "Duplicate depth",
        summary: "The old map expands too much",
        description: "Nested cards obscure the active object",
        status: "open",
        priority: "high",
      },
      {
        planned_in_stage_id: "stage",
        about: { object_type: "stage", object_id: "stage" },
      },
    ),
    objectV3(
      "work",
      "work-requirement",
      {
        title: "Implement model",
        summary: "Build a pure projection",
        description: "Create explorerModel.ts",
        status: "in_progress",
        priority: "normal",
      },
      {
        handles: { object_type: "requirement", object_id: "requirement" },
      },
    ),
    objectV3(
      "work",
      "work-issue",
      {
        title: "Remove recursive map",
        summary: "Retire the old projection",
        description: "Delete ProjectViewMap after migration",
        status: "pending",
        priority: "normal",
      },
      { handles: { object_type: "issue", object_id: "issue-stage" } },
    ),
    objectV3("plan", "plan-unbound", {
      title: "Unbound follow-up",
      description: "A plan without a Goal",
      status: "draft",
    }),
    objectV3("requirement", "requirement-unplanned", {
      title: "Unplanned requirement",
      description: "Not assigned to a Stage",
      status: "proposed",
      priority: "normal",
    }),
    objectV3(
      "issue",
      "issue-related",
      {
        title: "Plan-level concern",
        summary: "Attached directly to the Plan",
        description: "This Issue remains structurally unplanned",
        status: "open",
        priority: "normal",
      },
      { about: { object_type: "plan", object_id: "plan" } },
    ),
    objectV3(
      "issue",
      "issue-cycle-a",
      {
        title: "Cycle A",
        description: "References the other Issue",
        status: "open",
        priority: "low",
      },
      { about: { object_type: "issue", object_id: "issue-cycle-b" } },
    ),
    objectV3(
      "issue",
      "issue-cycle-b",
      {
        title: "Cycle B",
        description: "References the first Issue",
        status: "open",
        priority: "low",
      },
      { about: { object_type: "issue", object_id: "issue-cycle-a" } },
    ),
    objectV3("role", "role", {
      name: "Desktop steward",
      summary: "Owns the Desktop experience",
      purpose: "Keep the UI coherent",
      responsibilities: ["Review implementation"],
      boundaries: ["Do not change domain facts"],
      active: true,
    }),
    objectV3(
      "resource",
      "resource",
      {
        name: "Design system",
        resource_kind: "repository",
        summary: "Shared Desktop components",
        guide_document_id: "doc-guide",
      },
      {},
      [
        { type: "document", document_id: "doc-guide", mode: "live" },
        {
          type: "document",
          document_id: "doc-guide",
          mode: "pinned",
          document_revision: 2,
        },
      ],
    ),
  ]);
  const catalog = indexProjectDocumentCatalog([
    document("doc-guide", "Design guide", "How to use shared UI"),
    document("doc-live", "Explorer brief", "The current implementation brief"),
    document(
      "doc-pinned",
      "CURRENT TITLE MUST NOT APPEAR",
      "Current summary must not label revision 3",
    ),
  ]);
  return {
    model: buildProjectViewExplorerModel({ view, documentCatalog: catalog }),
    view,
  };
}

test("summary projection reads only the explicit source-owned summary", () => {
  const { model } = fixture();
  assert.equal(
    projectViewObjectSummary(model.objectsById.get("plan")),
    "Implement the approved design",
  );
  assert.equal(
    projectViewObjectSummary(model.objectsById.get("plan-unbound")),
    undefined,
  );
});

test("outline keeps canonical and reference occurrences finite and distinct", () => {
  const { model } = fixture();
  assert.deepEqual(
    model.root.children
      .filter((node) => node.kind === "group")
      .map((node) => node.label),
    ["Goals", "Roles", "Resources", "Unplaced Objects", "Related Resources"],
  );
  const planAliasKey = issueAboutOccurrenceKey("plan", "issue-related");
  const planAlias = model.nodesByOccurrence.get(planAliasKey);
  assert.equal(planAlias?.kind, "object_reference");
  assert.equal(Object.hasOwn(planAlias, "children"), false);
  const cycleAlias = model.nodesByOccurrence.get(
    issueAboutOccurrenceKey("issue-cycle-a", "issue-cycle-b"),
  );
  assert.equal(cycleAlias?.kind, "object_reference");
  assert.equal(Object.hasOwn(cycleAlias, "children"), false);
  assert.ok(model.nodesByOccurrence.size < 100);
  assert.ok(
    model.nodesByOccurrence.has(canonicalObjectOccurrenceKey("issue-cycle-a")),
  );
  assert.ok(model.nodesByOccurrence.has(planAliasKey));
});

test("outline and Main never borrow current metadata for a pinned revision", () => {
  const { model } = fixture();
  const pinnedKey = documentContextOccurrenceKey({
    ownerObjectId: "plan",
    documentId: "doc-pinned",
    mode: "pinned",
    revision: 3,
  });
  const pinnedNode = model.nodesByOccurrence.get(pinnedKey);
  assert.equal(pinnedNode?.kind, "document");
  assert.equal(pinnedNode?.metadataState, "coordinate_only");
  assert.match(pinnedNode?.title ?? "", /pinned revision 3/);
  assert.doesNotMatch(pinnedNode?.title ?? "", /CURRENT TITLE/);

  const page = buildProjectViewExplorerPage(model, {
    kind: "object",
    objectId: "plan",
  });
  assert.equal(page.kind, "object");
  const pinnedSummary = page.documents.find(
    (item) => item.documentId === "doc-pinned",
  );
  assert.equal(pinnedSummary?.summary, undefined);
  assert.equal(pinnedSummary?.metadataState, "coordinate_only");
});

test("Main projects exactly one structural layer and deduplicates related items", () => {
  const { model } = fixture();
  const planPage = buildProjectViewExplorerPage(model, {
    kind: "object",
    objectId: "plan",
  });
  assert.equal(planPage.kind, "object");
  assert.deepEqual(
    planPage.structuralGroups.map((group) => [
      group.label,
      group.items.map((item) => item.objectId),
    ]),
    [["Stages", ["stage"]]],
  );
  assert.deepEqual(
    planPage.relatedIssues.map((item) => item.objectId),
    ["issue-related"],
  );
  assert.deepEqual(
    planPage.relatedResources.map((item) => item.objectId),
    ["resource"],
  );
  assert.deepEqual(
    planPage.documents.map((item) => item.documentId),
    ["doc-live", "doc-pinned"],
  );
  assert.equal(
    planPage.structuralGroups.some((group) =>
      group.items.some((item) => item.objectId === "requirement"),
    ),
    false,
  );

  const stagePage = buildProjectViewExplorerPage(model, {
    kind: "object",
    objectId: "stage",
  });
  assert.equal(stagePage.kind, "object");
  assert.deepEqual(
    stagePage.structuralGroups.map((group) => [
      group.label,
      group.items.map((item) => item.objectId),
    ]),
    [
      ["Requirements", ["requirement"]],
      ["Issues", ["issue-stage"]],
    ],
  );
  assert.deepEqual(stagePage.relatedIssues, []);
  assert.equal(
    stagePage.structuralGroups.some((group) =>
      group.items.some((item) => item.objectId === "work-issue"),
    ),
    false,
  );

  const requirementPage = buildProjectViewExplorerPage(model, {
    kind: "object",
    objectId: "requirement",
  });
  assert.equal(requirementPage.kind, "object");
  assert.deepEqual(
    requirementPage.structuralGroups[0].items.map((item) => item.objectId),
    ["work-requirement"],
  );
});

test("Project and Resource pages preserve grouping and coordinate-aware deduplication", () => {
  const { model } = fixture();
  const projectPage = buildProjectViewExplorerPage(model);
  assert.equal(projectPage.kind, "object");
  assert.deepEqual(
    projectPage.structuralGroups.map((group) => group.label),
    ["Goals", "Roles", "Resources", "Unplaced Objects"],
  );
  assert.equal(projectPage.relatedResources.length, 0);
  assert.deepEqual(
    projectPage.structuralGroups
      .find((group) => group.label === "Unplaced Objects")
      ?.items.map((item) => item.objectId),
    [
      "plan-unbound",
      "requirement-unplanned",
      "issue-cycle-a",
      "issue-cycle-b",
      "issue-related",
    ],
  );

  const resourcePage = buildProjectViewExplorerPage(model, {
    kind: "object",
    objectId: "resource",
  });
  assert.equal(resourcePage.kind, "object");
  assert.deepEqual(
    resourcePage.documents.map((item) => [
      item.typeLabel,
      item.documentId,
      item.documentRevision,
    ]),
    [
      ["Guide Document", "doc-guide", undefined],
      ["Pinned Document", "doc-guide", 2],
    ],
  );
});

test("parent navigation follows the selected occurrence instead of object identity", () => {
  const { model } = fixture();
  const canonicalIssue = buildProjectViewExplorerPage(model, {
    kind: "object",
    objectId: "issue-related",
  });
  assert.equal(canonicalIssue.kind, "object");
  assert.deepEqual(canonicalIssue.parent, {
    objectId: "profile",
    title: "Carryforth",
  });

  const aliasIssue = buildProjectViewExplorerPage(model, {
    kind: "object",
    objectId: "issue-related",
    via: issueAboutOccurrenceKey("plan", "issue-related"),
  });
  assert.equal(aliasIssue.kind, "object");
  assert.deepEqual(aliasIssue.parent, {
    objectId: "plan",
    title: "Desktop delivery",
  });

  const unboundPlan = buildProjectViewExplorerPage(model, {
    kind: "object",
    objectId: "plan-unbound",
  });
  assert.equal(unboundPlan.kind, "object");
  assert.equal(unboundPlan.parent?.objectId, "profile");
  const root = buildProjectViewExplorerPage(model);
  assert.equal(root.kind, "object");
  assert.equal(root.parent, undefined);
});

test("Document selection keeps its exact owner occurrence and Documents route", () => {
  const { model } = fixture();
  const via = documentContextOccurrenceKey({
    ownerObjectId: "plan",
    documentId: "doc-pinned",
    mode: "pinned",
    revision: 3,
  });
  const page = buildProjectViewExplorerPage(model, {
    kind: "document",
    documentId: "doc-pinned",
    revision: 3,
    via,
  });
  assert.equal(page.kind, "document");
  assert.deepEqual(page.parent, {
    objectId: "plan",
    title: "Desktop delivery",
  });
  assert.deepEqual(page.openInDocumentsSearch, {
    document: "doc-pinned",
    revision: 3,
  });

  const guidePage = buildProjectViewExplorerPage(model, {
    kind: "document",
    documentId: "doc-guide",
    via: resourceGuideOccurrenceKey("resource", "doc-guide"),
  });
  assert.equal(guidePage.kind, "document");
  assert.equal(guidePage.parent.objectId, "resource");
});

test("invalid identities fall back to Project and invalid occurrences canonicalize", () => {
  const { model } = fixture();
  assert.deepEqual(
    resolveProjectViewExplorerSelection(model, {
      kind: "object",
      objectId: "missing",
    }),
    {
      kind: "object",
      object: model.view.profile,
      occurrenceKey: canonicalObjectOccurrenceKey("profile"),
      resolution: "fallback",
    },
  );
  const canonicalized = resolveProjectViewExplorerSelection(model, {
    kind: "object",
    objectId: "issue-related",
    via: issueAboutOccurrenceKey("stage", "issue-stage"),
  });
  assert.equal(canonicalized.kind, "object");
  assert.equal(
    canonicalized.occurrenceKey,
    canonicalObjectOccurrenceKey("issue-related"),
  );
  assert.equal(canonicalized.resolution, "canonicalized");
});

test("Explorer fails closed when a verified Issue reference target is missing", () => {
  const { view } = fixture();
  view.issueReferencesByTarget.missing = [
    { objectType: "issue", objectId: "issue-related" },
  ];
  assert.throws(
    () => buildProjectViewExplorerModel({ view }),
    /could not resolve object missing/,
  );
});
