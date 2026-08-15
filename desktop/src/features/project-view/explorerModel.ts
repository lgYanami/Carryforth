import {
  projectViewObjectSummary,
  projectViewObjectTitle,
  projectViewObjectTypeLabel,
} from "@/features/project-view/model";
import type { ProjectDocumentListItem } from "@/shared/api/tauriProjectDocument";
import type {
  ProjectView,
  ProjectViewIssue,
  ProjectViewObject,
  ProjectViewObjectOf,
  ProjectViewObjectType,
  ProjectViewPlan,
  ProjectViewRequirement,
  ProjectViewStage,
} from "@/shared/api/tauriProjectView";
import { ProjectViewIntegrityError } from "@/shared/api/tauriProjectView";

export type ProjectViewOutlineObjectNode = {
  kind: "object";
  occurrenceKey: string;
  relation: "root" | "structural";
  object: ProjectViewObject;
  title: string;
  children: ProjectViewOutlineNode[];
};

export type ProjectViewOutlineObjectReferenceNode = {
  kind: "object_reference";
  occurrenceKey: string;
  relation: "about" | "context";
  ownerObjectId: string;
  object: ProjectViewObjectOf<"issue"> | ProjectViewObjectOf<"resource">;
  title: string;
};

export type ProjectViewOutlineDocumentNode = {
  kind: "document";
  occurrenceKey: string;
  relation: "context" | "resource_guide";
  ownerObjectId: string;
  documentId: string;
  mode: "live" | "pinned";
  documentRevision?: number;
  title: string;
  metadataState: "verified" | "coordinate_only" | "unavailable";
};

export type ProjectViewOutlineGroupNode = {
  kind: "group";
  occurrenceKey: string;
  label: string;
  children: ProjectViewOutlineNode[];
};

export type ProjectViewOutlineNode =
  | ProjectViewOutlineObjectNode
  | ProjectViewOutlineObjectReferenceNode
  | ProjectViewOutlineDocumentNode
  | ProjectViewOutlineGroupNode;

export type ProjectViewObjectSummaryItem = {
  kind: "object";
  occurrenceKey: string;
  objectId: string;
  objectType: ProjectViewObjectType;
  typeLabel: string;
  title: string;
  summary?: string;
};

export type ProjectViewDocumentSummaryItem = {
  kind: "document";
  occurrenceKey: string;
  ownerObjectId: string;
  relation: "context" | "resource_guide";
  documentId: string;
  mode: "live" | "pinned";
  documentRevision?: number;
  typeLabel: "Document" | "Pinned Document" | "Guide Document";
  title: string;
  summary?: string;
  metadataState: "verified" | "coordinate_only" | "unavailable";
};

export type ProjectViewSummaryGroup = {
  label: string;
  items: ProjectViewObjectSummaryItem[];
};

export type ProjectViewExplorerParent = {
  objectId: string;
  title: string;
};

export type ProjectViewExplorerSelection =
  | { kind: "object"; objectId: string; via?: string }
  | {
      kind: "document";
      documentId: string;
      revision?: number;
      via?: string;
    };

export type ResolvedProjectViewExplorerSelection =
  | {
      kind: "object";
      object: ProjectViewObject;
      occurrenceKey: string;
      resolution: "requested" | "canonicalized" | "fallback";
    }
  | {
      kind: "document";
      node: ProjectViewOutlineDocumentNode;
      occurrenceKey: string;
      resolution: "requested" | "canonicalized";
    };

export type ProjectViewObjectPage = {
  kind: "object";
  currentObject: ProjectViewObject;
  occurrenceKey: string;
  parent?: ProjectViewExplorerParent;
  structuralGroups: ProjectViewSummaryGroup[];
  relatedIssues: ProjectViewObjectSummaryItem[];
  relatedResources: ProjectViewObjectSummaryItem[];
  documents: ProjectViewDocumentSummaryItem[];
};

export type ProjectViewDocumentPage = {
  kind: "document";
  occurrenceKey: string;
  parent: ProjectViewExplorerParent;
  coordinate: {
    documentId: string;
    revision?: number;
    mode: "live" | "pinned";
    relation: "context" | "resource_guide";
    ownerObjectId: string;
  };
  openInDocumentsSearch: {
    document: string;
    revision?: number;
  };
};

export type ProjectViewExplorerPage =
  | ProjectViewObjectPage
  | ProjectViewDocumentPage;

export type ProjectViewExplorerModel = {
  view: ProjectView;
  root: ProjectViewOutlineObjectNode;
  objectsById: ReadonlyMap<string, ProjectViewObject>;
  nodesByOccurrence: ReadonlyMap<string, ProjectViewOutlineNode>;
  canonicalOccurrenceByObjectId: ReadonlyMap<string, string>;
  documentOccurrencesByCoordinate: ReadonlyMap<
    string,
    ProjectViewOutlineDocumentNode[]
  >;
  parentObjectByOccurrence: ReadonlyMap<string, ProjectViewExplorerParent>;
  structuralGroupsByObjectId: ReadonlyMap<
    string,
    Array<{ label: string; objects: ProjectViewObject[] }>
  >;
  documentCatalog: ReadonlyMap<string, ProjectDocumentListItem>;
};

type HierarchyIndex = {
  plansByGoal: Map<string, ProjectViewPlan[]>;
  stagesByPlan: Map<string, ProjectViewStage[]>;
  requirementsByStage: Map<string, ProjectViewRequirement[]>;
  issuesByStage: Map<string, ProjectViewIssue[]>;
  worksByTarget: Map<string, Array<ProjectViewObjectOf<"work">>>;
};

export function canonicalObjectOccurrenceKey(objectId: string): string {
  return `object:${objectId}:canonical`;
}

export function issueAboutOccurrenceKey(
  targetObjectId: string,
  issueId: string,
): string {
  return `issue-about:${targetObjectId}:${issueId}`;
}

export function resourceContextOccurrenceKey(
  ownerObjectId: string,
  resourceId: string,
): string {
  return `resource-context:${ownerObjectId}:${resourceId}`;
}

export function documentContextOccurrenceKey(input: {
  ownerObjectId: string;
  documentId: string;
  mode: "live" | "pinned";
  revision?: number;
}): string {
  const suffix = input.mode === "pinned" ? `pinned:${input.revision}` : "live";
  return `document-context:${input.ownerObjectId}:${input.documentId}:${suffix}`;
}

export function resourceGuideOccurrenceKey(
  resourceId: string,
  documentId: string,
): string {
  return `resource-guide:${resourceId}:${documentId}`;
}

function documentCoordinateKey(documentId: string, revision?: number): string {
  return `document:${documentId}:${revision ?? "current"}`;
}

function groupOccurrenceKey(ownerObjectId: string, group: string): string {
  return `group:${ownerObjectId}:${group}`;
}

function shortDocumentId(documentId: string): string {
  return documentId.length > 12 ? documentId.slice(0, 8) : documentId;
}

function safeDocumentTitle(
  documentId: string,
  catalogItem?: ProjectDocumentListItem,
): string {
  const title = catalogItem?.title.trim();
  return title || `Document ${shortDocumentId(documentId)}`;
}

function pinnedDocumentTitle(documentId: string, revision?: number): string {
  return `Document ${shortDocumentId(documentId)} · pinned revision ${revision ?? "?"}`;
}

function requireObject(
  objectsById: ReadonlyMap<string, ProjectViewObject>,
  objectId: string,
  expectedType?: ProjectViewObjectType,
): ProjectViewObject {
  const object = objectsById.get(objectId);
  if (!object || (expectedType && object.objectType !== expectedType)) {
    throw new ProjectViewIntegrityError(
      `Project View Explorer could not resolve ${expectedType ?? "object"} ${objectId}`,
    );
  }
  return object;
}

function indexHierarchy(view: ProjectView): {
  hierarchy: HierarchyIndex;
  objectsById: Map<string, ProjectViewObject>;
} {
  const objectsById = new Map<string, ProjectViewObject>();
  const plansByGoal = new Map<string, ProjectViewPlan[]>();
  const stagesByPlan = new Map<string, ProjectViewStage[]>();
  const requirementsByStage = new Map<string, ProjectViewRequirement[]>();
  const issuesByStage = new Map<string, ProjectViewIssue[]>();
  const worksByTarget = new Map<string, Array<ProjectViewObjectOf<"work">>>();
  const add = (object: ProjectViewObject) => objectsById.set(object.id, object);
  const addRequirement = (entry: ProjectViewRequirement) => {
    add(entry.requirement);
    worksByTarget.set(entry.requirement.id, entry.works);
    entry.works.forEach(add);
  };
  const addIssue = (entry: ProjectViewIssue) => {
    add(entry.issue);
    worksByTarget.set(entry.issue.id, entry.works);
    entry.works.forEach(add);
  };
  const addStage = (entry: ProjectViewStage) => {
    add(entry.stage);
    requirementsByStage.set(entry.stage.id, entry.requirements);
    issuesByStage.set(entry.stage.id, entry.issues);
    entry.requirements.forEach(addRequirement);
    entry.issues.forEach(addIssue);
  };
  const addPlan = (entry: ProjectViewPlan) => {
    add(entry.plan);
    stagesByPlan.set(entry.plan.id, entry.stages);
    entry.stages.forEach(addStage);
  };

  add(view.profile);
  for (const goal of view.goals) {
    add(goal.goal);
    plansByGoal.set(goal.goal.id, goal.plans);
    goal.plans.forEach(addPlan);
  }
  view.unboundPlans.forEach(addPlan);
  view.unplannedRequirements.forEach(addRequirement);
  view.unplannedIssues.forEach(addIssue);
  view.roles.forEach(add);
  view.resources.forEach(add);
  return {
    objectsById,
    hierarchy: {
      plansByGoal,
      stagesByPlan,
      requirementsByStage,
      issuesByStage,
      worksByTarget,
    },
  };
}

function objectSummaryItem(
  object: ProjectViewObject,
  occurrenceKey: string,
): ProjectViewObjectSummaryItem {
  return {
    kind: "object",
    occurrenceKey,
    objectId: object.id,
    objectType: object.objectType,
    typeLabel: projectViewObjectTypeLabel(object.objectType),
    title: projectViewObjectTitle(object),
    summary: projectViewObjectSummary(object),
  };
}

function nonEmptyGroup(
  occurrenceKey: string,
  label: string,
  children: ProjectViewOutlineNode[],
): ProjectViewOutlineGroupNode | undefined {
  return children.length > 0
    ? { kind: "group", occurrenceKey, label, children }
    : undefined;
}

function compactGroups(
  groups: Array<ProjectViewOutlineGroupNode | undefined>,
): ProjectViewOutlineGroupNode[] {
  return groups.filter(
    (group): group is ProjectViewOutlineGroupNode => group !== undefined,
  );
}

function buildStructuralGroups(
  view: ProjectView,
  hierarchy: HierarchyIndex,
): Map<string, Array<{ label: string; objects: ProjectViewObject[] }>> {
  const groups = new Map<
    string,
    Array<{ label: string; objects: ProjectViewObject[] }>
  >();
  groups.set(view.profile.id, [
    { label: "Goals", objects: view.goals.map((entry) => entry.goal) },
    { label: "Roles", objects: [...view.roles] },
    { label: "Resources", objects: [...view.resources] },
    {
      label: "Unplaced Objects",
      objects: [
        ...view.unboundPlans.map((entry) => entry.plan),
        ...view.unplannedRequirements.map((entry) => entry.requirement),
        ...view.unplannedIssues.map((entry) => entry.issue),
      ],
    },
  ]);
  for (const goal of view.goals) {
    groups.set(goal.goal.id, [
      { label: "Plans", objects: goal.plans.map((entry) => entry.plan) },
    ]);
  }
  for (const object of hierarchy.stagesByPlan.keys()) {
    groups.set(object, [
      {
        label: "Stages",
        objects: (hierarchy.stagesByPlan.get(object) ?? []).map(
          (entry) => entry.stage,
        ),
      },
    ]);
  }
  for (const stageId of hierarchy.requirementsByStage.keys()) {
    groups.set(stageId, [
      {
        label: "Requirements",
        objects: (hierarchy.requirementsByStage.get(stageId) ?? []).map(
          (entry) => entry.requirement,
        ),
      },
      {
        label: "Issues",
        objects: (hierarchy.issuesByStage.get(stageId) ?? []).map(
          (entry) => entry.issue,
        ),
      },
    ]);
  }
  for (const [targetId, works] of hierarchy.worksByTarget) {
    groups.set(targetId, [{ label: "Work", objects: [...works] }]);
  }
  return groups;
}

function documentNode(input: {
  ownerObjectId: string;
  documentId: string;
  mode: "live" | "pinned";
  documentRevision?: number;
  relation: "context" | "resource_guide";
  documentCatalog: ReadonlyMap<string, ProjectDocumentListItem>;
}): ProjectViewOutlineDocumentNode {
  const catalogItem = input.documentCatalog.get(input.documentId);
  const pinned = input.mode === "pinned";
  return {
    kind: "document",
    occurrenceKey:
      input.relation === "resource_guide"
        ? resourceGuideOccurrenceKey(input.ownerObjectId, input.documentId)
        : documentContextOccurrenceKey({
            ownerObjectId: input.ownerObjectId,
            documentId: input.documentId,
            mode: input.mode,
            revision: input.documentRevision,
          }),
    relation: input.relation,
    ownerObjectId: input.ownerObjectId,
    documentId: input.documentId,
    mode: input.mode,
    documentRevision: input.documentRevision,
    title: pinned
      ? pinnedDocumentTitle(input.documentId, input.documentRevision)
      : safeDocumentTitle(input.documentId, catalogItem),
    metadataState: pinned
      ? "coordinate_only"
      : catalogItem
        ? "verified"
        : "unavailable",
  };
}

function buildReferenceGroups(input: {
  object: ProjectViewObject;
  view: ProjectView;
  objectsById: ReadonlyMap<string, ProjectViewObject>;
  documentCatalog: ReadonlyMap<string, ProjectDocumentListItem>;
}): ProjectViewOutlineGroupNode[] {
  const { object, view, objectsById, documentCatalog } = input;
  const issueNodes = (view.issueReferencesByTarget[object.id] ?? []).map(
    (reference): ProjectViewOutlineObjectReferenceNode => {
      const issue = requireObject(objectsById, reference.objectId, "issue");
      return {
        kind: "object_reference",
        occurrenceKey: issueAboutOccurrenceKey(object.id, issue.id),
        relation: "about",
        ownerObjectId: object.id,
        object: issue as ProjectViewObjectOf<"issue">,
        title: projectViewObjectTitle(issue),
      };
    },
  );
  const resourceNodes = (object.contextReferences ?? [])
    .filter((reference) => reference.referenceType === "resource")
    .map((reference): ProjectViewOutlineObjectReferenceNode => {
      const resource = requireObject(
        objectsById,
        reference.resourceId,
        "resource",
      );
      return {
        kind: "object_reference",
        occurrenceKey: resourceContextOccurrenceKey(object.id, resource.id),
        relation: "context",
        ownerObjectId: object.id,
        object: resource as ProjectViewObjectOf<"resource">,
        title: projectViewObjectTitle(resource),
      };
    });
  const documentNodes: ProjectViewOutlineDocumentNode[] = [];
  if (object.objectType === "resource") {
    documentNodes.push(
      documentNode({
        ownerObjectId: object.id,
        documentId: object.data.guideDocumentId,
        mode: "live",
        relation: "resource_guide",
        documentCatalog,
      }),
    );
  }
  for (const reference of object.contextReferences ?? []) {
    if (reference.referenceType !== "document") continue;
    documentNodes.push(
      documentNode({
        ownerObjectId: object.id,
        documentId: reference.documentId,
        mode: reference.mode,
        documentRevision: reference.documentRevision,
        relation: "context",
        documentCatalog,
      }),
    );
  }
  return compactGroups([
    nonEmptyGroup(
      groupOccurrenceKey(object.id, "related-issues"),
      "Related Issues",
      issueNodes,
    ),
    nonEmptyGroup(
      groupOccurrenceKey(object.id, "related-resources"),
      "Related Resources",
      resourceNodes,
    ),
    nonEmptyGroup(
      groupOccurrenceKey(object.id, "documents"),
      "Documents",
      documentNodes,
    ),
  ]);
}

function indexOutline(root: ProjectViewOutlineObjectNode): {
  nodesByOccurrence: Map<string, ProjectViewOutlineNode>;
  canonicalOccurrenceByObjectId: Map<string, string>;
  documentOccurrencesByCoordinate: Map<
    string,
    ProjectViewOutlineDocumentNode[]
  >;
  parentObjectByOccurrence: Map<string, ProjectViewExplorerParent>;
} {
  const nodesByOccurrence = new Map<string, ProjectViewOutlineNode>();
  const canonicalOccurrenceByObjectId = new Map<string, string>();
  const documentOccurrencesByCoordinate = new Map<
    string,
    ProjectViewOutlineDocumentNode[]
  >();
  const parentObjectByOccurrence = new Map<string, ProjectViewExplorerParent>();
  const visit = (
    node: ProjectViewOutlineNode,
    parentObject?: ProjectViewObject,
  ) => {
    nodesByOccurrence.set(node.occurrenceKey, node);
    if (parentObject) {
      parentObjectByOccurrence.set(node.occurrenceKey, {
        objectId: parentObject.id,
        title: projectViewObjectTitle(parentObject),
      });
    }
    if (node.kind === "document") {
      const key = documentCoordinateKey(node.documentId, node.documentRevision);
      const occurrences = documentOccurrencesByCoordinate.get(key) ?? [];
      occurrences.push(node);
      documentOccurrencesByCoordinate.set(key, occurrences);
      return;
    }
    if (node.kind === "object_reference") return;
    const nextParent = node.kind === "object" ? node.object : parentObject;
    if (node.kind === "object") {
      canonicalOccurrenceByObjectId.set(node.object.id, node.occurrenceKey);
    }
    node.children.forEach((child) => {
      visit(child, nextParent);
    });
  };
  visit(root);
  return {
    nodesByOccurrence,
    canonicalOccurrenceByObjectId,
    documentOccurrencesByCoordinate,
    parentObjectByOccurrence,
  };
}

export function indexProjectDocumentCatalog(
  documents: ProjectDocumentListItem[] = [],
): Map<string, ProjectDocumentListItem> {
  return new Map(documents.map((document) => [document.documentId, document]));
}

export function buildProjectViewExplorerModel(input: {
  view: ProjectView;
  documentCatalog?: ReadonlyMap<string, ProjectDocumentListItem>;
}): ProjectViewExplorerModel {
  const documentCatalog = input.documentCatalog ?? new Map();
  const { view } = input;
  const { hierarchy, objectsById } = indexHierarchy(view);
  for (const [targetId, references] of Object.entries(
    view.issueReferencesByTarget,
  )) {
    requireObject(objectsById, targetId);
    references.forEach((reference) => {
      requireObject(objectsById, reference.objectId, "issue");
    });
  }
  const structuralGroupsByObjectId = buildStructuralGroups(view, hierarchy);
  const buildObjectNode = (
    object: ProjectViewObject,
    relation: "root" | "structural" = "structural",
  ): ProjectViewOutlineObjectNode => {
    let structuralGroups: ProjectViewOutlineGroupNode[];
    if (object.objectType === "project_profile") {
      const unplacedGroups = compactGroups([
        nonEmptyGroup(
          groupOccurrenceKey(object.id, "unbound-plans"),
          "Unbound Plans",
          view.unboundPlans.map((entry) => buildObjectNode(entry.plan)),
        ),
        nonEmptyGroup(
          groupOccurrenceKey(object.id, "unplanned-requirements"),
          "Unplanned Requirements",
          view.unplannedRequirements.map((entry) =>
            buildObjectNode(entry.requirement),
          ),
        ),
        nonEmptyGroup(
          groupOccurrenceKey(object.id, "unplanned-issues"),
          "Unplanned Issues",
          view.unplannedIssues.map((entry) => buildObjectNode(entry.issue)),
        ),
      ]);
      structuralGroups = compactGroups([
        nonEmptyGroup(
          groupOccurrenceKey(object.id, "goals"),
          "Goals",
          view.goals.map((entry) => buildObjectNode(entry.goal)),
        ),
        nonEmptyGroup(
          groupOccurrenceKey(object.id, "roles"),
          "Roles",
          view.roles.map((role) => buildObjectNode(role)),
        ),
        nonEmptyGroup(
          groupOccurrenceKey(object.id, "resources"),
          "Resources",
          view.resources.map((resource) => buildObjectNode(resource)),
        ),
        nonEmptyGroup(
          groupOccurrenceKey(object.id, "unplaced"),
          "Unplaced Objects",
          unplacedGroups,
        ),
      ]);
    } else {
      structuralGroups = (structuralGroupsByObjectId.get(object.id) ?? [])
        .map((group, index) =>
          nonEmptyGroup(
            groupOccurrenceKey(
              object.id,
              `${group.label.toLowerCase().replaceAll(" ", "-")}-${index}`,
            ),
            group.label,
            group.objects.map((child) => buildObjectNode(child)),
          ),
        )
        .filter(
          (group): group is ProjectViewOutlineGroupNode => group !== undefined,
        );
    }
    return {
      kind: "object",
      occurrenceKey: canonicalObjectOccurrenceKey(object.id),
      relation,
      object,
      title: projectViewObjectTitle(object),
      children: [
        ...structuralGroups,
        ...buildReferenceGroups({
          object,
          view,
          objectsById,
          documentCatalog,
        }),
      ],
    };
  };
  const root = buildObjectNode(view.profile, "root");
  const outline = indexOutline(root);
  return {
    view,
    root,
    objectsById,
    structuralGroupsByObjectId,
    documentCatalog,
    ...outline,
  };
}

function nodeMatchesObject(
  node: ProjectViewOutlineNode | undefined,
  objectId: string,
): boolean {
  return (
    (node?.kind === "object" || node?.kind === "object_reference") &&
    node.object.id === objectId
  );
}

function nodeMatchesDocument(
  node: ProjectViewOutlineNode | undefined,
  documentId: string,
  revision?: number,
): node is ProjectViewOutlineDocumentNode {
  return (
    node?.kind === "document" &&
    node.documentId === documentId &&
    node.documentRevision === revision
  );
}

export function resolveProjectViewExplorerSelection(
  model: ProjectViewExplorerModel,
  selection?: ProjectViewExplorerSelection,
): ResolvedProjectViewExplorerSelection {
  if (selection?.kind === "object") {
    const object = model.objectsById.get(selection.objectId);
    if (object) {
      const viaNode = selection.via
        ? model.nodesByOccurrence.get(selection.via)
        : undefined;
      if (selection.via && nodeMatchesObject(viaNode, object.id)) {
        return {
          kind: "object",
          object,
          occurrenceKey: selection.via,
          resolution: "requested",
        };
      }
      return {
        kind: "object",
        object,
        occurrenceKey:
          model.canonicalOccurrenceByObjectId.get(object.id) ??
          canonicalObjectOccurrenceKey(object.id),
        resolution: selection.via ? "canonicalized" : "requested",
      };
    }
  } else if (selection?.kind === "document") {
    const viaNode = selection.via
      ? model.nodesByOccurrence.get(selection.via)
      : undefined;
    if (
      selection.via &&
      nodeMatchesDocument(viaNode, selection.documentId, selection.revision)
    ) {
      return {
        kind: "document",
        node: viaNode,
        occurrenceKey: viaNode.occurrenceKey,
        resolution: "requested",
      };
    }
    const occurrences = model.documentOccurrencesByCoordinate.get(
      documentCoordinateKey(selection.documentId, selection.revision),
    );
    const node = occurrences?.[0];
    if (node) {
      return {
        kind: "document",
        node,
        occurrenceKey: node.occurrenceKey,
        resolution: selection.via ? "canonicalized" : "requested",
      };
    }
  }
  return {
    kind: "object",
    object: model.view.profile,
    occurrenceKey: canonicalObjectOccurrenceKey(model.view.profile.id),
    resolution: selection ? "fallback" : "requested",
  };
}

function documentSummaryItem(
  node: ProjectViewOutlineDocumentNode,
  documentCatalog: ReadonlyMap<string, ProjectDocumentListItem>,
): ProjectViewDocumentSummaryItem {
  const catalogItem = documentCatalog.get(node.documentId);
  return {
    kind: "document",
    occurrenceKey: node.occurrenceKey,
    ownerObjectId: node.ownerObjectId,
    relation: node.relation,
    documentId: node.documentId,
    mode: node.mode,
    documentRevision: node.documentRevision,
    typeLabel:
      node.relation === "resource_guide"
        ? "Guide Document"
        : node.mode === "pinned"
          ? "Pinned Document"
          : "Document",
    title: node.title,
    summary:
      node.mode === "pinned"
        ? undefined
        : catalogItem?.summary?.trim() || undefined,
    metadataState: node.metadataState,
  };
}

function referenceNodes(
  model: ProjectViewExplorerModel,
  objectId: string,
  groupLabel: string,
): ProjectViewOutlineNode[] {
  const canonicalKey = model.canonicalOccurrenceByObjectId.get(objectId);
  const canonical = canonicalKey
    ? model.nodesByOccurrence.get(canonicalKey)
    : undefined;
  if (canonical?.kind !== "object") return [];
  const group = canonical.children.find(
    (child) => child.kind === "group" && child.label === groupLabel,
  );
  return group?.kind === "group" ? group.children : [];
}

export function buildProjectViewExplorerPage(
  model: ProjectViewExplorerModel,
  selection?: ProjectViewExplorerSelection,
): ProjectViewExplorerPage {
  const resolved = resolveProjectViewExplorerSelection(model, selection);
  const parent = model.parentObjectByOccurrence.get(resolved.occurrenceKey);
  if (resolved.kind === "document") {
    if (!parent) {
      throw new ProjectViewIntegrityError(
        `Document occurrence ${resolved.occurrenceKey} has no parent object`,
      );
    }
    return {
      kind: "document",
      occurrenceKey: resolved.occurrenceKey,
      parent,
      coordinate: {
        documentId: resolved.node.documentId,
        revision: resolved.node.documentRevision,
        mode: resolved.node.mode,
        relation: resolved.node.relation,
        ownerObjectId: resolved.node.ownerObjectId,
      },
      openInDocumentsSearch: {
        document: resolved.node.documentId,
        revision: resolved.node.documentRevision,
      },
    };
  }

  const structuralGroups = (
    model.structuralGroupsByObjectId.get(resolved.object.id) ?? []
  )
    .map((group) => ({
      label: group.label,
      items: group.objects.map((object) =>
        objectSummaryItem(object, canonicalObjectOccurrenceKey(object.id)),
      ),
    }))
    .filter((group) => group.items.length > 0);
  const structuralIds = new Set(
    structuralGroups.flatMap((group) =>
      group.items.map((item) => item.objectId),
    ),
  );
  const relatedIssues = referenceNodes(
    model,
    resolved.object.id,
    "Related Issues",
  )
    .filter(
      (node): node is ProjectViewOutlineObjectReferenceNode =>
        node.kind === "object_reference" && node.object.objectType === "issue",
    )
    .filter((node) => !structuralIds.has(node.object.id))
    .map((node) => objectSummaryItem(node.object, node.occurrenceKey));
  const relatedResources = referenceNodes(
    model,
    resolved.object.id,
    "Related Resources",
  )
    .filter(
      (node): node is ProjectViewOutlineObjectReferenceNode =>
        node.kind === "object_reference" &&
        node.object.objectType === "resource",
    )
    .filter((node) => !structuralIds.has(node.object.id))
    .map((node) => objectSummaryItem(node.object, node.occurrenceKey));
  const documentNodes = referenceNodes(
    model,
    resolved.object.id,
    "Documents",
  ).filter(
    (node): node is ProjectViewOutlineDocumentNode => node.kind === "document",
  );
  const guideLiveIds = new Set(
    documentNodes
      .filter((node) => node.relation === "resource_guide")
      .map((node) => node.documentId),
  );
  const documents = documentNodes
    .filter(
      (node) =>
        node.relation === "resource_guide" ||
        node.mode === "pinned" ||
        !guideLiveIds.has(node.documentId),
    )
    .map((node) => documentSummaryItem(node, model.documentCatalog));
  return {
    kind: "object",
    currentObject: resolved.object,
    occurrenceKey: resolved.occurrenceKey,
    parent,
    structuralGroups,
    relatedIssues,
    relatedResources,
    documents,
  };
}

/** Return the prior occurrence's nearest-to-farthest object ancestors. */
export function projectViewExplorerFallbackObjectIds(
  model: ProjectViewExplorerModel,
  page: ProjectViewExplorerPage,
): string[] {
  const objectIds: string[] = [];
  const seen = new Set<string>();
  let parent = page.parent;
  while (parent && !seen.has(parent.objectId)) {
    seen.add(parent.objectId);
    objectIds.push(parent.objectId);
    const canonicalOccurrence = model.canonicalOccurrenceByObjectId.get(
      parent.objectId,
    );
    parent = canonicalOccurrence
      ? model.parentObjectByOccurrence.get(canonicalOccurrence)
      : undefined;
  }
  return objectIds;
}

/** Resolve the structural parent used after deleting an object from any occurrence. */
export function projectViewCanonicalParent(
  model: ProjectViewExplorerModel,
  objectId: string,
): ProjectViewExplorerParent | undefined {
  const occurrence = model.canonicalOccurrenceByObjectId.get(objectId);
  return occurrence
    ? model.parentObjectByOccurrence.get(occurrence)
    : undefined;
}
