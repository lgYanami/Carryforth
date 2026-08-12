import * as React from "react";

import {
  clampProjectContextPanelContentWidth,
  createProjectContextWorkspacePanelState,
  deriveProjectContextWorkspacePanelLayout,
  PROJECT_CONTEXT_BASE_ROOT_REM_PX,
  PROJECT_CONTEXT_PANEL_DEFAULT_CONTENT_REM,
  projectContextWorkspacePanelReducer,
  type ProjectContextWorkspaceGraphTarget,
  type ProjectContextWorkspaceAnnouncementEvent,
  type ProjectContextWorkspaceOpenOrigin,
  type ProjectContextWorkspacePanelAction,
  type ProjectContextWorkspacePaneRenderContext,
  type ProjectContextWorkspacePanelState,
  type ProjectContextWorkspacePresentation,
} from "@/features/project-context/workspacePanelModel";
import type { ProjectContextSemanticToolStatus } from "@/features/project-context/ui/ProjectContextToolRail";
import {
  ProjectContextToolsPanel,
  type ProjectContextToolPaneRenderer,
} from "@/features/project-context/ui/ProjectContextToolsPanel";
import { cn } from "@/shared/lib/cn";

export const PROJECT_CONTEXT_WORKSPACE_TEST_IDS = {
  root: "project-context-workspace",
  canvasSlot: "project-context-canvas-slot",
  announcement: "project-context-workspace-announcement",
} as const;

export type ProjectContextWorkspaceCanvasInsets = {
  top: number;
  right: number;
  bottom: number;
  left: number;
};

export type ProjectContextWorkspaceCanvasRenderContext = {
  dispatchPanel: React.Dispatch<ProjectContextWorkspacePanelAction>;
  externalCanvasInsets: Partial<ProjectContextWorkspaceCanvasInsets>;
  fitSuspended: boolean;
  panelExpanded: boolean;
  panelState: ProjectContextWorkspacePanelState;
  presentation: ProjectContextWorkspacePresentation;
  registerSelectionOpenIntent: (
    expectedSelectionKey: string,
    graphTarget: ProjectContextWorkspaceGraphTarget,
  ) => void;
  rejectSelectionOpenIntent: () => void;
};

export type {
  ProjectContextWorkspaceAnnouncementEvent,
  ProjectContextWorkspacePaneRenderContext,
};

export type ProjectContextWorkspaceProps = {
  /** One Human-facing event; keyed events can repeat the same spoken text. */
  announcement?: string | ProjectContextWorkspaceAnnouncementEvent;
  className?: string;
  detailsUnavailableReason?: string;
  onCloseSelection: () => void;
  onRestoreCanvasFocus?: () => void;
  onRestoreGraphTargetFocus?: (
    target: ProjectContextWorkspaceGraphTarget,
  ) => boolean | undefined;
  renderCanvas: (
    context: ProjectContextWorkspaceCanvasRenderContext,
  ) => React.ReactNode;
  renderPane: ProjectContextToolPaneRenderer;
  rootRemPxOverride?: number;
  selectionKey: string | null;
  semanticStatus?: ProjectContextSemanticToolStatus;
  workspaceWidthPxOverride?: number;
};

function readRootRemPx(): number {
  if (typeof window === "undefined" || typeof document === "undefined") {
    return PROJECT_CONTEXT_BASE_ROOT_REM_PX;
  }
  const value = Number.parseFloat(
    window.getComputedStyle(document.documentElement).fontSize,
  );
  return Number.isFinite(value) && value > 0
    ? value
    : PROJECT_CONTEXT_BASE_ROOT_REM_PX;
}

function useRootRemPx(override?: number) {
  const [measured, setMeasured] = React.useState(readRootRemPx);

  React.useLayoutEffect(() => {
    if (override !== undefined) return;
    const update = () => setMeasured(readRootRemPx());
    const observer = new MutationObserver(update);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class", "style"],
    });
    window.addEventListener("resize", update);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", update);
    };
  }, [override]);

  return override ?? measured;
}

function useWorkspaceWidth(
  ref: React.RefObject<HTMLDivElement | null>,
  override?: number,
) {
  const [measured, setMeasured] = React.useState(0);

  React.useLayoutEffect(() => {
    if (override !== undefined) return;
    const element = ref.current;
    if (!element) return;
    const update = () => setMeasured(element.getBoundingClientRect().width);
    update();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", update);
      return () => window.removeEventListener("resize", update);
    }
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, [override, ref]);

  return override ?? measured;
}

function useProjectContextPanelResize({
  dispatch,
  panelContentWidthPx,
  rootRemPx,
  workspaceWidthPx,
}: {
  dispatch: React.Dispatch<ProjectContextWorkspacePanelAction>;
  panelContentWidthPx: number;
  rootRemPx: number;
  workspaceWidthPx: number;
}) {
  const cleanupRef = React.useRef<(() => void) | null>(null);

  React.useEffect(() => () => cleanupRef.current?.(), []);

  const onResizeStart = React.useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      event.preventDefault();
      cleanupRef.current?.();
      const startX = event.clientX;
      const startWidth = panelContentWidthPx;
      const previousCursor = document.body.style.cursor;
      const previousUserSelect = document.body.style.userSelect;
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";

      const cleanup = () => {
        document.body.style.cursor = previousCursor;
        document.body.style.userSelect = previousUserSelect;
        window.removeEventListener("pointermove", handlePointerMove);
        window.removeEventListener("pointerup", cleanup);
        window.removeEventListener("pointercancel", cleanup);
        cleanupRef.current = null;
      };
      const handlePointerMove = (moveEvent: PointerEvent) => {
        dispatch({
          type: "panel_content_width_changed",
          widthPx: clampProjectContextPanelContentWidth({
            panelContentWidthPx: startWidth + startX - moveEvent.clientX,
            preserveDockedCanvas: true,
            rootRemPx,
            workspaceWidthPx,
          }),
        });
      };

      cleanupRef.current = cleanup;
      window.addEventListener("pointermove", handlePointerMove);
      window.addEventListener("pointerup", cleanup, { once: true });
      window.addEventListener("pointercancel", cleanup, { once: true });
    },
    [dispatch, panelContentWidthPx, rootRemPx, workspaceWidthPx],
  );

  const defaultWidthPx = PROJECT_CONTEXT_PANEL_DEFAULT_CONTENT_REM * rootRemPx;
  return {
    canReset: Math.abs(panelContentWidthPx - defaultWidthPx) > 0.5,
    onResetWidth: React.useCallback(
      () =>
        dispatch({
          type: "panel_content_width_changed",
          widthPx: defaultWidthPx,
        }),
      [defaultWidthPx, dispatch],
    ),
    onResizeStart,
  };
}

function normalizeAnnouncement(
  announcement?: string | ProjectContextWorkspaceAnnouncementEvent,
): ProjectContextWorkspaceAnnouncementEvent | null {
  if (typeof announcement === "string") {
    const message = announcement.trim();
    return message ? { key: `legacy:${message}`, message } : null;
  }
  if (!announcement) return null;
  const key = announcement.key.trim();
  const message = announcement.message.trim();
  return key && message ? { key: `event:${key}`, message } : null;
}

function useDeduplicatedAnnouncement(
  announcement?: string | ProjectContextWorkspaceAnnouncementEvent,
) {
  const lastAnnouncementKeyRef = React.useRef<string | undefined>(undefined);
  const [liveAnnouncement, setLiveAnnouncement] =
    React.useState<ProjectContextWorkspaceAnnouncementEvent | null>(null);

  React.useEffect(() => {
    const next = normalizeAnnouncement(announcement);
    if (!next) {
      lastAnnouncementKeyRef.current = undefined;
      setLiveAnnouncement(null);
      return;
    }
    if (next.key === lastAnnouncementKeyRef.current) return;
    lastAnnouncementKeyRef.current = next.key;
    setLiveAnnouncement(next);
  }, [announcement]);

  return liveAnnouncement;
}

/** Full-height graph substrate with one responsive, Community-scoped tool surface. */
export function ProjectContextWorkspace({
  announcement,
  className,
  detailsUnavailableReason,
  onCloseSelection,
  onRestoreCanvasFocus,
  onRestoreGraphTargetFocus,
  renderCanvas,
  renderPane,
  rootRemPxOverride,
  selectionKey,
  semanticStatus = "idle",
  workspaceWidthPxOverride,
}: ProjectContextWorkspaceProps) {
  const rootRemPx = useRootRemPx(rootRemPxOverride);
  const [panelState, dispatchPanel] = React.useReducer(
    projectContextWorkspacePanelReducer,
    {
      initialSelectionKey: selectionKey,
      panelContentWidthPx:
        PROJECT_CONTEXT_PANEL_DEFAULT_CONTENT_REM * rootRemPx,
    },
    createProjectContextWorkspacePanelState,
  );
  const workspaceRef = React.useRef<HTMLDivElement>(null);
  const workspaceWidthPx = useWorkspaceWidth(
    workspaceRef,
    workspaceWidthPxOverride,
  );
  const layout = deriveProjectContextWorkspacePanelLayout({
    expanded: panelState.expanded,
    panelContentWidthPx: panelState.panelContentWidthPx,
    rootRemPx,
    workspaceWidthPx,
  });
  const resize = useProjectContextPanelResize({
    dispatch: dispatchPanel,
    panelContentWidthPx: layout.panelContentWidthPx,
    rootRemPx,
    workspaceWidthPx,
  });
  const structureButtonRef = React.useRef<HTMLButtonElement>(null);
  const semanticButtonRef = React.useRef<HTMLButtonElement>(null);
  const detailsButtonRef = React.useRef<HTMLButtonElement>(null);
  const buttonRefs = React.useMemo(
    () => ({
      structure: structureButtonRef,
      semantic: semanticButtonRef,
      details: detailsButtonRef,
    }),
    [],
  );
  const liveAnnouncement = useDeduplicatedAnnouncement(announcement);
  const announcementRegion = (
    <div
      aria-atomic="true"
      aria-live="polite"
      className="sr-only"
      data-testid={PROJECT_CONTEXT_WORKSPACE_TEST_IDS.announcement}
      role="status"
    >
      {liveAnnouncement ? (
        <span key={liveAnnouncement.key}>{liveAnnouncement.message}</span>
      ) : null}
    </div>
  );
  const modalPresentation =
    layout.presentation === "drawer" || layout.presentation === "sheet";
  const viewportActionFrameRef = React.useRef<number | null>(null);

  React.useEffect(
    () => () => {
      if (viewportActionFrameRef.current !== null) {
        cancelAnimationFrame(viewportActionFrameRef.current);
      }
    },
    [],
  );

  React.useLayoutEffect(() => {
    dispatchPanel({ type: "selection_observed", selectionKey });
  }, [selectionKey]);

  const restoreFocus = React.useCallback(
    (origin: ProjectContextWorkspaceOpenOrigin) => {
      if (origin?.kind === "rail") {
        buttonRefs[origin.tool].current?.focus({ preventScroll: true });
        return;
      }
      if (origin?.kind === "graph" && onRestoreGraphTargetFocus) {
        const restored = onRestoreGraphTargetFocus(origin.target);
        if (restored !== false) return;
      }
      if (selectionKey && detailsButtonRef.current) {
        detailsButtonRef.current.focus({ preventScroll: true });
        return;
      }
      onRestoreCanvasFocus?.();
    },
    [buttonRefs, onRestoreCanvasFocus, onRestoreGraphTargetFocus, selectionKey],
  );

  const registerSelectionOpenIntent = React.useCallback(
    (
      expectedSelectionKey: string,
      graphTarget: ProjectContextWorkspaceGraphTarget,
    ) => {
      dispatchPanel({
        type: "selection_open_intent",
        expectedSelectionKey,
        origin: { kind: "graph", target: graphTarget },
      });
    },
    [],
  );
  const rejectSelectionOpenIntent = React.useCallback(
    () => dispatchPanel({ type: "selection_open_rejected" }),
    [],
  );
  const closeModalForViewportAction = React.useCallback(
    (afterClose?: () => void) => {
      if (layout.presentation !== "drawer" && layout.presentation !== "sheet") {
        afterClose?.();
        return;
      }

      dispatchPanel({ type: "collapse" });
      if (viewportActionFrameRef.current !== null) {
        cancelAnimationFrame(viewportActionFrameRef.current);
      }
      viewportActionFrameRef.current = requestAnimationFrame(() => {
        viewportActionFrameRef.current = null;
        onRestoreCanvasFocus?.();
        afterClose?.();
      });
    },
    [layout.presentation, onRestoreCanvasFocus],
  );

  // The collapsed Rail sits 0.75rem from the edge and needs the same gap on
  // its canvas side. Docked content is already excluded from canvas geometry.
  const collapsedRailInsetPx = layout.railWidthPx + rootRemPx * 1.5;
  const canvasContext =
    React.useMemo<ProjectContextWorkspaceCanvasRenderContext>(
      () => ({
        dispatchPanel,
        externalCanvasInsets:
          layout.presentation === "collapsed"
            ? { right: collapsedRailInsetPx }
            : { right: 0 },
        fitSuspended: layout.fitSuspended,
        panelExpanded: panelState.expanded,
        panelState,
        presentation: layout.presentation,
        registerSelectionOpenIntent,
        rejectSelectionOpenIntent,
      }),
      [
        collapsedRailInsetPx,
        layout.fitSuspended,
        layout.presentation,
        panelState,
        registerSelectionOpenIntent,
        rejectSelectionOpenIntent,
      ],
    );

  return (
    <div
      className={cn(
        "relative flex h-full min-h-0 min-w-0 flex-1 overflow-hidden",
        className,
      )}
      data-panel-expanded={panelState.expanded ? "true" : "false"}
      data-presentation={layout.presentation}
      data-testid={PROJECT_CONTEXT_WORKSPACE_TEST_IDS.root}
      ref={workspaceRef}
    >
      <div
        className="relative min-h-0 min-w-0 flex-1 overflow-hidden"
        data-testid={PROJECT_CONTEXT_WORKSPACE_TEST_IDS.canvasSlot}
      >
        {renderCanvas(canvasContext)}
      </div>
      <ProjectContextToolsPanel
        buttonRefs={buttonRefs}
        closeModalForViewportAction={closeModalForViewportAction}
        detailsUnavailableReason={detailsUnavailableReason}
        dispatch={dispatchPanel}
        layout={layout}
        modalAnnouncementRegion={
          modalPresentation ? announcementRegion : undefined
        }
        onCloseSelection={onCloseSelection}
        onResetWidth={resize.canReset ? resize.onResetWidth : undefined}
        onResizeStart={
          layout.presentation === "docked" ? resize.onResizeStart : undefined
        }
        onRestoreFocus={restoreFocus}
        renderPane={renderPane}
        selectionAvailable={selectionKey !== null}
        semanticStatus={semanticStatus}
        state={panelState}
      />
      {modalPresentation ? null : announcementRegion}
    </div>
  );
}
