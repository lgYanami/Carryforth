export type ProjectContextWorkspaceTool = "structure" | "semantic" | "details";

export type ProjectContextWorkspaceReturnTool = Exclude<
  ProjectContextWorkspaceTool,
  "details"
>;

export type ProjectContextWorkspaceGraphTarget = {
  kind: "coordinate" | "edge";
  key: string;
};

/** One content-free Human-facing event for the workspace live region. */
export type ProjectContextWorkspaceAnnouncementEvent = {
  key: string;
  message: string;
};

export type ProjectContextWorkspaceOpenOrigin =
  | { kind: "rail"; tool: ProjectContextWorkspaceTool }
  | { kind: "graph"; target: ProjectContextWorkspaceGraphTarget }
  | { kind: "route" }
  | null;

export type ProjectContextWorkspaceSelectionIntent = {
  expectedSelectionKey: string;
  origin: Exclude<ProjectContextWorkspaceOpenOrigin, { kind: "route" } | null>;
};

export type ProjectContextWorkspacePanelState = {
  expanded: boolean;
  activeTool: ProjectContextWorkspaceTool;
  returnTool: ProjectContextWorkspaceReturnTool | null;
  returnExpanded: boolean;
  panelContentWidthPx: number;
  observedSelectionKey: string | null;
  pendingSelectionOrigin: ProjectContextWorkspaceSelectionIntent | null;
  openOrigin: ProjectContextWorkspaceOpenOrigin;
};

export type ProjectContextWorkspacePanelAction =
  | { type: "tool_toggled"; tool: ProjectContextWorkspaceTool }
  | { type: "collapse" }
  | {
      type: "selection_open_intent";
      expectedSelectionKey: string;
      origin: ProjectContextWorkspaceSelectionIntent["origin"];
    }
  | { type: "selection_open_rejected" }
  | { type: "selection_observed"; selectionKey: string | null }
  | { type: "panel_content_width_changed"; widthPx: number }
  | {
      type: "reset";
      initialSelectionKey?: string | null;
      panelContentWidthPx?: number;
    };

export type ProjectContextWorkspacePresentation =
  | "collapsed"
  | "docked"
  | "drawer"
  | "sheet";

export type ProjectContextWorkspacePanelLayout = {
  presentation: ProjectContextWorkspacePresentation;
  railWidthPx: number;
  panelContentWidthPx: number;
  assemblyWidthPx: number;
  overlayWidthPx: number;
  minimumCanvasWidthPx: number;
  sheetBoundaryPx: number;
  dockThresholdPx: number;
  fitSuspended: boolean;
};

export type ProjectContextWorkspacePaneRenderContext = {
  presentation: ProjectContextWorkspacePresentation;
  /**
   * Call only after establishing the authoritative Fit/Focus request.
   * Modal modes close first and invoke the callback on the next frame.
   */
  closeModalForViewportAction: (afterClose?: () => void) => void;
};

export const PROJECT_CONTEXT_PANEL_DEFAULT_CONTENT_REM = 27.5;
export const PROJECT_CONTEXT_PANEL_MIN_CONTENT_REM = 22.5;
export const PROJECT_CONTEXT_PANEL_MAX_CONTENT_REM = 35;
export const PROJECT_CONTEXT_TOOL_RAIL_WIDTH_REM = 3;
export const PROJECT_CONTEXT_MIN_CANVAS_WIDTH_REM = 40;
export const PROJECT_CONTEXT_SHEET_BOUNDARY_REM = 37.5;
export const PROJECT_CONTEXT_BASE_ROOT_REM_PX = 16;

const DEFAULT_PANEL_CONTENT_WIDTH_PX =
  PROJECT_CONTEXT_PANEL_DEFAULT_CONTENT_REM * PROJECT_CONTEXT_BASE_ROOT_REM_PX;

function finiteNonNegative(value: number, fallback = 0): number {
  return Number.isFinite(value) && value >= 0 ? value : fallback;
}

function validRootRemPx(value: number): number {
  return Number.isFinite(value) && value > 0
    ? value
    : PROJECT_CONTEXT_BASE_ROOT_REM_PX;
}

function validPanelWidthPx(value: number): number {
  return Number.isFinite(value) && value > 0
    ? value
    : DEFAULT_PANEL_CONTENT_WIDTH_PX;
}

/** Build the Community-scoped, in-memory presentation state. */
export function createProjectContextWorkspacePanelState({
  initialSelectionKey = null,
  panelContentWidthPx = DEFAULT_PANEL_CONTENT_WIDTH_PX,
}: {
  initialSelectionKey?: string | null;
  panelContentWidthPx?: number;
} = {}): ProjectContextWorkspacePanelState {
  const base: ProjectContextWorkspacePanelState = {
    expanded: false,
    activeTool: "structure",
    returnTool: null,
    returnExpanded: false,
    panelContentWidthPx: validPanelWidthPx(panelContentWidthPx),
    observedSelectionKey: null,
    pendingSelectionOrigin: null,
    openOrigin: null,
  };

  if (!initialSelectionKey) return base;

  return observeSelection(base, initialSelectionKey);
}

/** Give route selection variants one collision-free observation key. */
export function projectContextWorkspaceSelectionKey(selection: {
  kind: ProjectContextWorkspaceGraphTarget["kind"];
  key: ProjectContextWorkspaceGraphTarget["key"];
}): string {
  return `${selection.kind}:${selection.key}`;
}

function currentReturnState(state: ProjectContextWorkspacePanelState): {
  returnTool: ProjectContextWorkspaceReturnTool | null;
  returnExpanded: boolean;
} {
  if (state.activeTool === "details") {
    return {
      returnTool: state.returnTool,
      returnExpanded: state.returnExpanded,
    };
  }

  return {
    returnTool: state.activeTool,
    returnExpanded: state.expanded,
  };
}

function matchedIntentOrigin(
  state: ProjectContextWorkspacePanelState,
  selectionKey: string,
): ProjectContextWorkspaceOpenOrigin {
  return state.pendingSelectionOrigin?.expectedSelectionKey === selectionKey
    ? state.pendingSelectionOrigin.origin
    : { kind: "route" };
}

function observeSelection(
  state: ProjectContextWorkspacePanelState,
  selectionKey: string | null,
): ProjectContextWorkspacePanelState {
  const previousSelectionKey = state.observedSelectionKey;

  if (selectionKey === previousSelectionKey) {
    return state.pendingSelectionOrigin
      ? { ...state, pendingSelectionOrigin: null }
      : state;
  }

  if (selectionKey === null) {
    if (state.activeTool !== "details") {
      return {
        ...state,
        observedSelectionKey: null,
        pendingSelectionOrigin: null,
        returnTool: null,
        returnExpanded: false,
        openOrigin: null,
      };
    }

    return {
      ...state,
      expanded: state.returnExpanded,
      activeTool: state.returnTool ?? "structure",
      returnTool: null,
      returnExpanded: false,
      observedSelectionKey: null,
      pendingSelectionOrigin: null,
      openOrigin: null,
    };
  }

  const origin = matchedIntentOrigin(state, selectionKey);

  if (previousSelectionKey !== null && state.activeTool === "details") {
    const shouldReopen =
      !state.expanded && origin !== null && origin.kind !== "route";
    return {
      ...state,
      expanded: shouldReopen ? true : state.expanded,
      observedSelectionKey: selectionKey,
      pendingSelectionOrigin: null,
      openOrigin: shouldReopen ? origin : state.openOrigin,
    };
  }

  const returnState = currentReturnState(state);
  return {
    ...state,
    expanded: true,
    activeTool: "details",
    returnTool: returnState.returnTool,
    returnExpanded: returnState.returnExpanded,
    observedSelectionKey: selectionKey,
    pendingSelectionOrigin: null,
    openOrigin: origin,
  };
}

function toggleTool(
  state: ProjectContextWorkspacePanelState,
  tool: ProjectContextWorkspaceTool,
): ProjectContextWorkspacePanelState {
  if (tool === "details" && state.observedSelectionKey === null) {
    return state;
  }

  if (state.expanded && state.activeTool === tool) {
    return collapsePanel(state);
  }

  if (tool === "details") {
    const returnState = currentReturnState(state);
    return {
      ...state,
      expanded: true,
      activeTool: "details",
      returnTool: returnState.returnTool,
      returnExpanded: returnState.returnExpanded,
      pendingSelectionOrigin: null,
      openOrigin: { kind: "rail", tool },
    };
  }

  return {
    ...state,
    expanded: true,
    activeTool: tool,
    returnTool: state.observedSelectionKey === null ? null : tool,
    returnExpanded: state.observedSelectionKey !== null,
    pendingSelectionOrigin: null,
    openOrigin: { kind: "rail", tool },
  };
}

function collapsePanel(
  state: ProjectContextWorkspacePanelState,
): ProjectContextWorkspacePanelState {
  const returnTool =
    state.observedSelectionKey !== null && state.activeTool !== "details"
      ? state.activeTool
      : state.returnTool;

  return {
    ...state,
    expanded: false,
    returnTool,
    returnExpanded:
      state.observedSelectionKey !== null ? false : state.returnExpanded,
    pendingSelectionOrigin: null,
    openOrigin: null,
  };
}

/** Pure presentation reducer. Route state remains the selection fact owner. */
export function projectContextWorkspacePanelReducer(
  state: ProjectContextWorkspacePanelState,
  action: ProjectContextWorkspacePanelAction,
): ProjectContextWorkspacePanelState {
  switch (action.type) {
    case "tool_toggled":
      return toggleTool(state, action.tool);
    case "collapse":
      return collapsePanel(state);
    case "selection_open_intent":
      return action.expectedSelectionKey
        ? {
            ...state,
            pendingSelectionOrigin: {
              expectedSelectionKey: action.expectedSelectionKey,
              origin: action.origin,
            },
          }
        : state;
    case "selection_open_rejected":
      return state.pendingSelectionOrigin
        ? { ...state, pendingSelectionOrigin: null }
        : state;
    case "selection_observed":
      return observeSelection(state, action.selectionKey);
    case "panel_content_width_changed":
      return {
        ...state,
        panelContentWidthPx: validPanelWidthPx(action.widthPx),
      };
    case "reset":
      return createProjectContextWorkspacePanelState({
        initialSelectionKey: action.initialSelectionKey,
        panelContentWidthPx: action.panelContentWidthPx,
      });
  }
}

/** Clamp content width against rem bounds and, when requested, canvas space. */
export function clampProjectContextPanelContentWidth({
  panelContentWidthPx,
  preserveDockedCanvas = false,
  rootRemPx,
  workspaceWidthPx,
}: {
  panelContentWidthPx: number;
  preserveDockedCanvas?: boolean;
  rootRemPx: number;
  workspaceWidthPx?: number;
}): number {
  const remPx = validRootRemPx(rootRemPx);
  const minimum = PROJECT_CONTEXT_PANEL_MIN_CONTENT_REM * remPx;
  const maximum = PROJECT_CONTEXT_PANEL_MAX_CONTENT_REM * remPx;
  const requested = validPanelWidthPx(panelContentWidthPx);
  let upperBound = maximum;

  if (preserveDockedCanvas && workspaceWidthPx !== undefined) {
    const workspace = finiteNonNegative(workspaceWidthPx);
    const rail = PROJECT_CONTEXT_TOOL_RAIL_WIDTH_REM * remPx;
    const minimumCanvas = PROJECT_CONTEXT_MIN_CANVAS_WIDTH_REM * remPx;
    upperBound = Math.min(upperBound, workspace - rail - minimumCanvas);
  }

  // A workspace that cannot fit the minimum is a Drawer/Sheet concern. Keep
  // the saved content width valid instead of manufacturing a sub-minimum dock.
  return Math.min(Math.max(requested, minimum), Math.max(minimum, upperBound));
}

/** Derive collapsed/docked/drawer/sheet without persisting a media mode. */
export function deriveProjectContextWorkspacePanelLayout({
  expanded,
  panelContentWidthPx,
  rootRemPx,
  workspaceWidthPx,
}: {
  expanded: boolean;
  panelContentWidthPx: number;
  rootRemPx: number;
  workspaceWidthPx: number;
}): ProjectContextWorkspacePanelLayout {
  const remPx = validRootRemPx(rootRemPx);
  const workspace = finiteNonNegative(workspaceWidthPx);
  const railWidthPx = PROJECT_CONTEXT_TOOL_RAIL_WIDTH_REM * remPx;
  const minimumCanvasWidthPx = PROJECT_CONTEXT_MIN_CANVAS_WIDTH_REM * remPx;
  const sheetBoundaryPx = PROJECT_CONTEXT_SHEET_BOUNDARY_REM * remPx;
  const contentWidthPx = clampProjectContextPanelContentWidth({
    panelContentWidthPx,
    rootRemPx: remPx,
  });
  const assemblyWidthPx = railWidthPx + contentWidthPx;
  const dockThresholdPx = assemblyWidthPx + minimumCanvasWidthPx;

  let presentation: ProjectContextWorkspacePresentation = "collapsed";
  if (expanded) {
    if (workspace < sheetBoundaryPx) {
      presentation = "sheet";
    } else if (workspace >= dockThresholdPx) {
      presentation = "docked";
    } else {
      presentation = "drawer";
    }
  }

  const overlayWidthPx =
    presentation === "sheet"
      ? workspace
      : Math.max(0, Math.min(contentWidthPx, workspace - railWidthPx));

  return {
    presentation,
    railWidthPx,
    panelContentWidthPx: contentWidthPx,
    assemblyWidthPx,
    overlayWidthPx,
    minimumCanvasWidthPx,
    sheetBoundaryPx,
    dockThresholdPx,
    fitSuspended: presentation === "drawer" || presentation === "sheet",
  };
}
