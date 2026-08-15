import {
  Box,
  ChevronDown,
  ChevronRight,
  FileText,
  Folder,
  Link2,
  LocateFixed,
} from "lucide-react";
import * as React from "react";

import type {
  ProjectViewExplorerModel,
  ProjectViewExplorerSelection,
  ProjectViewOutlineNode,
} from "@/features/project-view/explorerModel";
import { projectViewObjectTypeLabel } from "@/features/project-view/model";
import {
  collapseProjectViewOutlineToCurrent,
  expandAllProjectViewOutline,
  expandProjectViewOutlineAncestors,
  indexProjectViewOutline,
  navigateProjectViewOutline,
  projectViewOutlineChildren,
  reconcileProjectViewOutlineExpanded,
  visibleProjectViewCurrentContainer,
  type ProjectViewOutlineNavigationKey,
} from "@/features/project-view/outlineState";
import { cn } from "@/shared/lib/cn";

export type ProjectViewOutlineHandle = {
  collapseAll: () => void;
  expandAll: () => void;
};

type ProjectViewOutlineProps = {
  currentOccurrenceKey: string;
  model: ProjectViewExplorerModel;
  onEscape?: () => void;
  onNavigate: (selection: ProjectViewExplorerSelection) => void;
};

function outlineNodeLabel(node: ProjectViewOutlineNode): string {
  return node.kind === "group" ? node.label : node.title;
}

function outlineNodeType(node: ProjectViewOutlineNode): string | undefined {
  if (node.kind === "object" || node.kind === "object_reference") {
    return projectViewObjectTypeLabel(node.object.objectType);
  }
  if (node.kind === "document") {
    if (node.relation === "resource_guide") return "Guide";
    return node.mode === "pinned" ? "Pinned" : "Document";
  }
  return undefined;
}

function outlineNodeDepth(
  parentKeyByKey: ReadonlyMap<string, string>,
  occurrenceKey: string,
): number {
  let depth = 1;
  let parentKey = parentKeyByKey.get(occurrenceKey);
  while (parentKey) {
    depth += 1;
    parentKey = parentKeyByKey.get(parentKey);
  }
  return depth;
}

function selectionForOutlineNode(
  node: ProjectViewOutlineNode,
): ProjectViewExplorerSelection | undefined {
  if (node.kind === "object") {
    return { kind: "object", objectId: node.object.id };
  }
  if (node.kind === "object_reference") {
    return {
      kind: "object",
      objectId: node.object.id,
      via: node.occurrenceKey,
    };
  }
  if (node.kind === "document") {
    return {
      kind: "document",
      documentId: node.documentId,
      revision: node.documentRevision,
      via: node.occurrenceKey,
    };
  }
  return undefined;
}

/** Accessible, occurrence-aware Project View tree with independent expansion state. */
export const ProjectViewOutline = React.forwardRef<
  ProjectViewOutlineHandle,
  ProjectViewOutlineProps
>(function ProjectViewOutline(
  { currentOccurrenceKey, model, onEscape, onNavigate },
  ref,
) {
  const index = React.useMemo(
    () => indexProjectViewOutline(model.root),
    [model.root],
  );
  const [expandedKeys, setExpandedKeys] = React.useState<Set<string>>(() =>
    expandProjectViewOutlineAncestors(
      index,
      currentOccurrenceKey,
      new Set([index.rootKey]),
    ),
  );
  const [focusedKey, setFocusedKey] = React.useState(currentOccurrenceKey);
  const previousCurrentKey = React.useRef(currentOccurrenceKey);
  const rowRefs = React.useRef(new Map<string, HTMLDivElement>());

  React.useImperativeHandle(
    ref,
    () => ({
      collapseAll() {
        setExpandedKeys(
          collapseProjectViewOutlineToCurrent(index, currentOccurrenceKey),
        );
        setFocusedKey(currentOccurrenceKey);
      },
      expandAll() {
        setExpandedKeys(expandAllProjectViewOutline(index));
      },
    }),
    [currentOccurrenceKey, index],
  );

  React.useEffect(() => {
    const currentChanged = previousCurrentKey.current !== currentOccurrenceKey;
    previousCurrentKey.current = currentOccurrenceKey;
    setExpandedKeys((current) => {
      const reconciled = reconcileProjectViewOutlineExpanded(index, current);
      return currentChanged
        ? expandProjectViewOutlineAncestors(
            index,
            currentOccurrenceKey,
            reconciled,
          )
        : reconciled;
    });
    setFocusedKey((current) =>
      index.nodesByKey.has(current) ? current : currentOccurrenceKey,
    );
  }, [currentOccurrenceKey, index]);

  const currentContainerKey = React.useMemo(
    () =>
      visibleProjectViewCurrentContainer(
        index,
        currentOccurrenceKey,
        expandedKeys,
      ),
    [currentOccurrenceKey, expandedKeys, index],
  );

  function focusRow(key: string) {
    window.requestAnimationFrame(() => rowRefs.current.get(key)?.focus());
  }

  function toggle(node: ProjectViewOutlineNode) {
    if (projectViewOutlineChildren(node).length === 0) return;
    setFocusedKey(node.occurrenceKey);
    setExpandedKeys((current) => {
      const next = reconcileProjectViewOutlineExpanded(index, current);
      if (next.has(node.occurrenceKey)) next.delete(node.occurrenceKey);
      else next.add(node.occurrenceKey);
      return next;
    });
    focusRow(node.occurrenceKey);
  }

  function activate(node: ProjectViewOutlineNode) {
    const selection = selectionForOutlineNode(node);
    if (selection) onNavigate(selection);
    else toggle(node);
  }

  function handleKeyDown(
    event: React.KeyboardEvent<HTMLDivElement>,
    node: ProjectViewOutlineNode,
  ) {
    if (event.key === "Escape" && onEscape) {
      event.preventDefault();
      onEscape();
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      activate(node);
      return;
    }
    const navigationKeys: ProjectViewOutlineNavigationKey[] = [
      "ArrowUp",
      "ArrowDown",
      "ArrowLeft",
      "ArrowRight",
      "Home",
      "End",
    ];
    if (
      !navigationKeys.includes(event.key as ProjectViewOutlineNavigationKey)
    ) {
      return;
    }
    event.preventDefault();
    const result = navigateProjectViewOutline({
      index,
      expandedKeys,
      focusedKey: node.occurrenceKey,
      key: event.key as ProjectViewOutlineNavigationKey,
    });
    setExpandedKeys(result.expandedKeys);
    setFocusedKey(result.focusedKey);
    focusRow(result.focusedKey);
  }

  function renderNode(node: ProjectViewOutlineNode): React.ReactNode {
    const children = projectViewOutlineChildren(node);
    const expandable = children.length > 0;
    const expanded = expandable && expandedKeys.has(node.occurrenceKey);
    const current = node.occurrenceKey === currentOccurrenceKey;
    const containsCurrent =
      !current && node.occurrenceKey === currentContainerKey;
    const type = outlineNodeType(node);
    const label = outlineNodeLabel(node);
    const depth = outlineNodeDepth(index.parentKeyByKey, node.occurrenceKey);
    return (
      <React.Fragment key={node.occurrenceKey}>
        <div
          aria-expanded={expandable ? expanded : undefined}
          aria-label={type ? `${type}: ${label}` : label}
          aria-level={depth}
          aria-selected={current}
          className={cn(
            "group mx-1 flex min-w-0 cursor-default items-center gap-1 rounded-md py-1.5 pr-2 text-xs outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
            current && "bg-primary/12 font-medium text-foreground",
            containsCurrent &&
              "border-l-2 border-primary bg-primary/5 font-medium",
            !current && !containsCurrent && "hover:bg-muted/60",
            node.kind === "group" && "text-muted-foreground",
          )}
          data-current-container={containsCurrent || undefined}
          data-occurrence-key={node.occurrenceKey}
          onClick={() => activate(node)}
          onKeyDown={(event) => handleKeyDown(event, node)}
          ref={(element) => {
            if (element) rowRefs.current.set(node.occurrenceKey, element);
            else rowRefs.current.delete(node.occurrenceKey);
          }}
          role="treeitem"
          style={{
            paddingInlineStart: `${8 + Math.min(depth - 1, 8) * 12}px`,
          }}
          tabIndex={focusedKey === node.occurrenceKey ? 0 : -1}
        >
          {expandable ? (
            <button
              aria-label={`${expanded ? "Collapse" : "Expand"} ${label}`}
              className="relative z-10 flex h-5 w-5 shrink-0 items-center justify-center rounded-sm hover:bg-muted focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
              onClick={(event) => {
                event.stopPropagation();
                toggle(node);
              }}
              tabIndex={-1}
              type="button"
            >
              {expanded ? (
                <ChevronDown className="h-3.5 w-3.5" />
              ) : (
                <ChevronRight className="h-3.5 w-3.5" />
              )}
            </button>
          ) : (
            <span className="h-5 w-5 shrink-0" />
          )}
          {node.kind === "group" ? (
            <Folder className="h-3.5 w-3.5 shrink-0" />
          ) : node.kind === "document" ? (
            <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          ) : node.kind === "object_reference" ? (
            <Link2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          ) : (
            <Box className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          )}
          {type ? (
            <span className="shrink-0 text-3xs font-semibold uppercase tracking-wide text-muted-foreground">
              {type}
            </span>
          ) : null}
          <span className="min-w-0 flex-1 truncate" title={label}>
            {label}
          </span>
          {current ? (
            <LocateFixed
              aria-label="Current item"
              className="h-3.5 w-3.5 shrink-0 text-primary"
            />
          ) : containsCurrent ? (
            <LocateFixed
              aria-label="Contains current item"
              className="h-3.5 w-3.5 shrink-0 text-primary"
            />
          ) : null}
        </div>
        {expanded ? (
          // biome-ignore lint/a11y/useSemanticElements: ARIA tree children require a role=group container.
          <div role="group">{children.map(renderNode)}</div>
        ) : null}
      </React.Fragment>
    );
  }

  return (
    <div
      aria-label="Project Outline"
      className="py-2"
      data-testid="project-view-outline-tree"
      role="tree"
    >
      {renderNode(model.root)}
    </div>
  );
});
