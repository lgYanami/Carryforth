import type { ProjectViewOutlineNode } from "@/features/project-view/explorerModel";

export type ProjectViewOutlineIndex = {
  rootKey: string;
  nodesByKey: ReadonlyMap<string, ProjectViewOutlineNode>;
  parentKeyByKey: ReadonlyMap<string, string>;
};

export type ProjectViewOutlineNavigationKey =
  | "ArrowUp"
  | "ArrowDown"
  | "ArrowLeft"
  | "ArrowRight"
  | "Home"
  | "End";

export type ProjectViewOutlineNavigationResult = {
  focusedKey: string;
  expandedKeys: Set<string>;
};

export function projectViewOutlineChildren(
  node: ProjectViewOutlineNode,
): ProjectViewOutlineNode[] {
  return node.kind === "object" || node.kind === "group" ? node.children : [];
}

export function indexProjectViewOutline(
  root: ProjectViewOutlineNode,
): ProjectViewOutlineIndex {
  const nodesByKey = new Map<string, ProjectViewOutlineNode>();
  const parentKeyByKey = new Map<string, string>();
  const visit = (node: ProjectViewOutlineNode, parentKey?: string) => {
    nodesByKey.set(node.occurrenceKey, node);
    if (parentKey) parentKeyByKey.set(node.occurrenceKey, parentKey);
    projectViewOutlineChildren(node).forEach((child) => {
      visit(child, node.occurrenceKey);
    });
  };
  visit(root);
  return { rootKey: root.occurrenceKey, nodesByKey, parentKeyByKey };
}

export function reconcileProjectViewOutlineExpanded(
  index: ProjectViewOutlineIndex,
  expandedKeys: ReadonlySet<string>,
): Set<string> {
  return new Set(
    [...expandedKeys].filter((key) => {
      const node = index.nodesByKey.get(key);
      return Boolean(node && projectViewOutlineChildren(node).length > 0);
    }),
  );
}

export function expandProjectViewOutlineAncestors(
  index: ProjectViewOutlineIndex,
  occurrenceKey: string,
  expandedKeys: ReadonlySet<string> = new Set(),
): Set<string> {
  const next = reconcileProjectViewOutlineExpanded(index, expandedKeys);
  let parentKey = index.parentKeyByKey.get(occurrenceKey);
  while (parentKey) {
    next.add(parentKey);
    parentKey = index.parentKeyByKey.get(parentKey);
  }
  return next;
}

export function visibleProjectViewOutlineNodes(
  index: ProjectViewOutlineIndex,
  expandedKeys: ReadonlySet<string>,
): ProjectViewOutlineNode[] {
  const root = index.nodesByKey.get(index.rootKey);
  if (!root) return [];
  const visible: ProjectViewOutlineNode[] = [];
  const visit = (node: ProjectViewOutlineNode) => {
    visible.push(node);
    if (!expandedKeys.has(node.occurrenceKey)) return;
    projectViewOutlineChildren(node).forEach(visit);
  };
  visit(root);
  return visible;
}

export function visibleProjectViewCurrentContainer(
  index: ProjectViewOutlineIndex,
  currentOccurrenceKey: string,
  expandedKeys: ReadonlySet<string>,
): string | undefined {
  const visibleKeys = new Set(
    visibleProjectViewOutlineNodes(index, expandedKeys).map(
      (node) => node.occurrenceKey,
    ),
  );
  let candidate: string | undefined = currentOccurrenceKey;
  while (candidate) {
    if (visibleKeys.has(candidate)) return candidate;
    candidate = index.parentKeyByKey.get(candidate);
  }
  return undefined;
}

export function navigateProjectViewOutline(input: {
  index: ProjectViewOutlineIndex;
  expandedKeys: ReadonlySet<string>;
  focusedKey: string;
  key: ProjectViewOutlineNavigationKey;
}): ProjectViewOutlineNavigationResult {
  const expandedKeys = reconcileProjectViewOutlineExpanded(
    input.index,
    input.expandedKeys,
  );
  const visible = visibleProjectViewOutlineNodes(input.index, expandedKeys);
  const focusedIndex = Math.max(
    0,
    visible.findIndex((node) => node.occurrenceKey === input.focusedKey),
  );
  const focused = visible[focusedIndex];
  if (!focused) {
    return { focusedKey: input.index.rootKey, expandedKeys };
  }
  if (input.key === "Home") {
    return { focusedKey: visible[0].occurrenceKey, expandedKeys };
  }
  if (input.key === "End") {
    return {
      focusedKey: visible[visible.length - 1].occurrenceKey,
      expandedKeys,
    };
  }
  if (input.key === "ArrowUp") {
    return {
      focusedKey: visible[Math.max(0, focusedIndex - 1)].occurrenceKey,
      expandedKeys,
    };
  }
  if (input.key === "ArrowDown") {
    return {
      focusedKey:
        visible[Math.min(visible.length - 1, focusedIndex + 1)].occurrenceKey,
      expandedKeys,
    };
  }
  const children = projectViewOutlineChildren(focused);
  if (input.key === "ArrowRight") {
    if (children.length === 0) {
      return { focusedKey: focused.occurrenceKey, expandedKeys };
    }
    if (!expandedKeys.has(focused.occurrenceKey)) {
      expandedKeys.add(focused.occurrenceKey);
      return { focusedKey: focused.occurrenceKey, expandedKeys };
    }
    return { focusedKey: children[0].occurrenceKey, expandedKeys };
  }
  if (children.length > 0 && expandedKeys.has(focused.occurrenceKey)) {
    expandedKeys.delete(focused.occurrenceKey);
    return { focusedKey: focused.occurrenceKey, expandedKeys };
  }
  return {
    focusedKey:
      input.index.parentKeyByKey.get(focused.occurrenceKey) ??
      focused.occurrenceKey,
    expandedKeys,
  };
}
