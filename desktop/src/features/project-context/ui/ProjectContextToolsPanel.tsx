import { ChevronRight, X } from "lucide-react";
import * as React from "react";

import type {
  ProjectContextWorkspaceOpenOrigin,
  ProjectContextWorkspacePanelAction,
  ProjectContextWorkspacePanelLayout,
  ProjectContextWorkspacePanelState,
  ProjectContextWorkspacePaneRenderContext,
  ProjectContextWorkspaceTool,
} from "@/features/project-context/workspacePanelModel";
import {
  ProjectContextToolRail,
  type ProjectContextSemanticToolStatus,
  type ProjectContextToolButtonRefs,
} from "@/features/project-context/ui/ProjectContextToolRail";
import { useEscapeKey } from "@/shared/hooks/useEscapeKey";
import { AuxiliaryPanel } from "@/shared/layout/AuxiliaryPanel";
import { cn } from "@/shared/lib/cn";
import { Button } from "@/shared/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/shared/ui/sheet";

export const PROJECT_CONTEXT_TOOLS_PANEL_ID = "project-context-tools-panel";

export const PROJECT_CONTEXT_TOOLS_PANEL_TEST_IDS = {
  panel: "project-context-tool-panel",
  collapse: "project-context-tools-collapse",
  detailsClose: "project-context-details-close",
  resizeHandle: "project-context-tools-resize-handle",
} as const;

const NON_RESTORABLE_PANEL_FOCUS = new Set<string>([
  PROJECT_CONTEXT_TOOLS_PANEL_TEST_IDS.collapse,
  PROJECT_CONTEXT_TOOLS_PANEL_TEST_IDS.detailsClose,
]);

function canRestorePanelFocus(element: HTMLElement): boolean {
  return (
    !element.matches(":disabled") &&
    element.tabIndex >= 0 &&
    element.closest("[hidden], [inert], [aria-hidden='true']") === null
  );
}

const TOOL_COPY: Record<
  ProjectContextWorkspaceTool,
  { title: string; description: string }
> = {
  structure: {
    title: "Structure",
    description: "Filter the verified graph and navigate its structure.",
  },
  semantic: {
    title: "Semantic paths",
    description: "Find paths related to a natural-language problem.",
  },
  details: {
    title: "Details",
    description: "Inspect the canonical selected graph object.",
  },
};

export type ProjectContextToolPaneRenderer = (
  tool: ProjectContextWorkspaceTool,
  context: ProjectContextWorkspacePaneRenderContext,
) => React.ReactNode;

export type ProjectContextToolsPanelProps = {
  buttonRefs?: ProjectContextToolButtonRefs;
  closeModalForViewportAction: ProjectContextWorkspacePaneRenderContext["closeModalForViewportAction"];
  detailsUnavailableReason?: string;
  dispatch: React.Dispatch<ProjectContextWorkspacePanelAction>;
  layout: ProjectContextWorkspacePanelLayout;
  /** The workspace's sole live region, rendered only inside modal portals. */
  modalAnnouncementRegion?: React.ReactNode;
  onCloseSelection: () => void;
  onResetWidth?: () => void;
  onResizeStart?: React.PointerEventHandler<HTMLButtonElement>;
  onRestoreFocus?: (origin: ProjectContextWorkspaceOpenOrigin) => void;
  renderPane: ProjectContextToolPaneRenderer;
  selectionAvailable: boolean;
  semanticStatus?: ProjectContextSemanticToolStatus;
  state: ProjectContextWorkspacePanelState;
};

function PaneSurface({
  activeTool,
  children,
  modal,
  onFocusCapture,
  onCloseSelection,
  onCollapse,
  panelId,
  presentation,
  surfaceRef,
}: {
  activeTool: ProjectContextWorkspaceTool;
  children: React.ReactNode;
  modal: boolean;
  onFocusCapture?: React.FocusEventHandler<HTMLElement>;
  onCloseSelection: () => void;
  onCollapse: () => void;
  panelId?: string;
  presentation: ProjectContextWorkspacePanelLayout["presentation"];
  surfaceRef?: React.Ref<HTMLElement>;
}) {
  const copy = TOOL_COPY[activeTool];
  const headingId = `${PROJECT_CONTEXT_TOOLS_PANEL_ID}-heading`;

  return (
    <section
      aria-labelledby={modal ? undefined : headingId}
      className="flex min-h-0 min-w-0 flex-1 flex-col bg-background"
      data-active-tool={activeTool}
      data-presentation={presentation}
      data-testid={PROJECT_CONTEXT_TOOLS_PANEL_TEST_IDS.panel}
      id={panelId}
      onFocusCapture={onFocusCapture}
      ref={surfaceRef}
      role={modal ? undefined : "region"}
    >
      <header className="flex min-h-12 shrink-0 items-center gap-2 border-b border-border/70 px-3 py-2">
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-base font-semibold" id={headingId}>
            {copy.title}
          </h2>
          <p className="truncate text-2xs text-muted-foreground">
            {copy.description}
          </p>
        </div>
        {activeTool === "details" ? (
          <Button
            aria-label="Close selection"
            data-testid={PROJECT_CONTEXT_TOOLS_PANEL_TEST_IDS.detailsClose}
            onClick={onCloseSelection}
            size="icon"
            type="button"
            variant="ghost"
          >
            <X />
          </Button>
        ) : null}
        <Button
          aria-label={`Collapse ${copy.title} tools`}
          data-testid={PROJECT_CONTEXT_TOOLS_PANEL_TEST_IDS.collapse}
          onClick={onCollapse}
          size="icon"
          type="button"
          variant="ghost"
        >
          <ChevronRight />
        </Button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain p-4">
        {activeTool === "details" ? (
          <div data-testid="project-context-inspector">{children}</div>
        ) : (
          children
        )}
      </div>
    </section>
  );
}

/**
 * Render the sole Project Context Rail and the one active tool pane.
 *
 * Modal modes deliberately unmount the external Rail and put the only Rail
 * inside Radix Sheet, preventing duplicate ids, controls, and tab stops.
 */
export function ProjectContextToolsPanel({
  buttonRefs,
  closeModalForViewportAction,
  detailsUnavailableReason,
  dispatch,
  layout,
  modalAnnouncementRegion,
  onCloseSelection,
  onResetWidth,
  onResizeStart,
  onRestoreFocus,
  renderPane,
  selectionAvailable,
  semanticStatus = "idle",
  state,
}: ProjectContextToolsPanelProps) {
  const activePane =
    state.expanded && (state.activeTool !== "details" || selectionAvailable)
      ? renderPane(state.activeTool, {
          closeModalForViewportAction,
          presentation: layout.presentation,
        })
      : null;
  const restoreFrameRef = React.useRef<number | null>(null);
  const openFocusFrameRef = React.useRef<number | null>(null);
  const paneSurfaceRef = React.useRef<HTMLElement>(null);
  const modalContentRef = React.useRef<HTMLDivElement>(null);
  const lastPaneFocusRef = React.useRef<
    Partial<Record<ProjectContextWorkspaceTool, string>>
  >({});
  const previousPresentationRef = React.useRef(layout.presentation);
  const wasExpandedRef = React.useRef(false);

  React.useEffect(
    () => () => {
      if (restoreFrameRef.current !== null) {
        cancelAnimationFrame(restoreFrameRef.current);
      }
      if (openFocusFrameRef.current !== null) {
        cancelAnimationFrame(openFocusFrameRef.current);
      }
    },
    [],
  );

  const rememberPaneFocus = React.useCallback(
    (event: React.FocusEvent<HTMLElement>) => {
      const testId = (event.target as HTMLElement).getAttribute("data-testid");
      if (testId && !NON_RESTORABLE_PANEL_FOCUS.has(testId)) {
        lastPaneFocusRef.current[state.activeTool] = testId;
      }
    },
    [state.activeTool],
  );

  const focusActiveSurface = React.useCallback(
    (root: HTMLElement | null) => {
      const rememberedTestId = lastPaneFocusRef.current[state.activeTool];
      const remembered = rememberedTestId
        ? Array.from(
            root?.querySelectorAll<HTMLElement>("[data-testid]") ?? [],
          ).find(
            (element) =>
              element.getAttribute("data-testid") === rememberedTestId &&
              canRestorePanelFocus(element),
          )
        : undefined;
      const fallback = buttonRefs?.[state.activeTool]?.current;
      const target = remembered ?? fallback;
      target?.focus({
        preventScroll: true,
      });
      if (target && document.activeElement !== target && fallback !== target) {
        fallback?.focus({ preventScroll: true });
      }
    },
    [buttonRefs, state.activeTool],
  );

  const scheduleOpenFocus = React.useCallback(
    (root: () => HTMLElement | null) => {
      if (openFocusFrameRef.current !== null) {
        cancelAnimationFrame(openFocusFrameRef.current);
      }
      openFocusFrameRef.current = requestAnimationFrame(() => {
        openFocusFrameRef.current = null;
        focusActiveSurface(root());
      });
    },
    [focusActiveSurface],
  );

  const scheduleFocusRestore = React.useCallback(() => {
    const origin = state.openOrigin;
    if (restoreFrameRef.current !== null) {
      cancelAnimationFrame(restoreFrameRef.current);
    }
    restoreFrameRef.current = requestAnimationFrame(() => {
      restoreFrameRef.current = null;
      onRestoreFocus?.(origin);
    });
  }, [onRestoreFocus, state.openOrigin]);

  React.useLayoutEffect(() => {
    const opened = !wasExpandedRef.current && state.expanded;
    const dockedMounted =
      state.expanded &&
      previousPresentationRef.current !== "docked" &&
      layout.presentation === "docked";
    wasExpandedRef.current = state.expanded;
    previousPresentationRef.current = layout.presentation;
    if (layout.presentation !== "docked" || (!opened && !dockedMounted)) {
      return;
    }
    if (state.openOrigin?.kind === "graph") {
      scheduleFocusRestore();
    } else {
      scheduleOpenFocus(() => paneSurfaceRef.current);
    }
  }, [
    layout.presentation,
    scheduleFocusRestore,
    scheduleOpenFocus,
    state.expanded,
    state.openOrigin,
  ]);

  const collapse = React.useCallback(() => {
    dispatch({ type: "collapse" });
    scheduleFocusRestore();
  }, [dispatch, scheduleFocusRestore]);

  const closeSelection = React.useCallback(() => {
    onCloseSelection();
    scheduleFocusRestore();
  }, [onCloseSelection, scheduleFocusRestore]);

  const toggleTool = React.useCallback(
    (tool: ProjectContextWorkspaceTool) => {
      const closesPanel = state.expanded && state.activeTool === tool;
      dispatch({ type: "tool_toggled", tool });
      if (closesPanel) scheduleFocusRestore();
    },
    [dispatch, scheduleFocusRestore, state.activeTool, state.expanded],
  );

  const handleDockedEscape = React.useCallback(() => {
    if (state.activeTool === "details") {
      closeSelection();
      return;
    }
    collapse();
  }, [closeSelection, collapse, state.activeTool]);
  useEscapeKey(handleDockedEscape, layout.presentation === "docked");

  const rail = (
    <ProjectContextToolRail
      activeTool={state.activeTool}
      buttonRefs={buttonRefs}
      detailsUnavailableReason={detailsUnavailableReason}
      expanded={state.expanded}
      onToolToggle={toggleTool}
      panelId={PROJECT_CONTEXT_TOOLS_PANEL_ID}
      selectionAvailable={selectionAvailable}
      semanticStatus={semanticStatus}
    />
  );

  if (layout.presentation === "collapsed") {
    return React.cloneElement(rail, {
      className: "absolute right-3 top-3 z-30 rounded-xl",
    });
  }

  if (layout.presentation === "docked") {
    return (
      <AuxiliaryPanel
        canResetWidth={onResetWidth != null}
        className="z-30"
        onClose={collapse}
        onResetWidth={onResetWidth}
        onResizeStart={onResizeStart}
        resizeHandleAriaLabel="Resize Project Context tools"
        resizeHandleTestId={PROJECT_CONTEXT_TOOLS_PANEL_TEST_IDS.resizeHandle}
        splitPaneClamp={false}
        widthPx={layout.assemblyWidthPx}
      >
        <div className="flex min-h-0 flex-1">
          {React.cloneElement(rail, {
            className: "h-full rounded-none border-y-0 border-l-0 shadow-none",
          })}
          <PaneSurface
            activeTool={state.activeTool}
            modal={false}
            onFocusCapture={rememberPaneFocus}
            onCloseSelection={closeSelection}
            onCollapse={collapse}
            panelId={PROJECT_CONTEXT_TOOLS_PANEL_ID}
            presentation={layout.presentation}
            surfaceRef={paneSurfaceRef}
          >
            {activePane}
          </PaneSurface>
        </div>
      </AuxiliaryPanel>
    );
  }

  const modalShellWidthPx =
    layout.presentation === "drawer"
      ? layout.railWidthPx + layout.overlayWidthPx
      : layout.overlayWidthPx;

  return (
    <Sheet
      onOpenChange={(open) => {
        if (!open) collapse();
      }}
      open
    >
      <SheetContent
        aria-modal="true"
        className={cn(
          "flex max-w-none! flex-row gap-0 overflow-hidden bg-background p-0 motion-reduce:animate-none motion-reduce:transition-none",
          layout.presentation === "sheet" && "w-full! border-l-0",
        )}
        data-presentation={layout.presentation}
        onCloseAutoFocus={(event) => event.preventDefault()}
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          scheduleOpenFocus(() => modalContentRef.current);
        }}
        onEscapeKeyDown={(event) => {
          if (event.defaultPrevented) return;
          event.preventDefault();
          collapse();
        }}
        showCloseButton={false}
        side="right"
        style={{ width: modalShellWidthPx }}
        ref={modalContentRef}
      >
        <SheetHeader className="sr-only">
          <SheetTitle>{TOOL_COPY[state.activeTool].title}</SheetTitle>
          <SheetDescription>
            {TOOL_COPY[state.activeTool].description}
          </SheetDescription>
        </SheetHeader>
        {modalAnnouncementRegion}
        {React.cloneElement(rail, {
          className: "h-full rounded-none border-y-0 border-l-0 shadow-none",
        })}
        <PaneSurface
          activeTool={state.activeTool}
          modal={true}
          onFocusCapture={rememberPaneFocus}
          onCloseSelection={closeSelection}
          onCollapse={collapse}
          panelId={PROJECT_CONTEXT_TOOLS_PANEL_ID}
          presentation={layout.presentation}
        >
          {activePane}
        </PaneSurface>
      </SheetContent>
    </Sheet>
  );
}
