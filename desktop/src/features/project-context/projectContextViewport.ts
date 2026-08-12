import { getViewportForBounds } from "@xyflow/react";

/** Screen-space chrome reserved around the Project Context canvas. */
export type ProjectContextCanvasInsets = Readonly<{
  top: number;
  right: number;
  bottom: number;
  left: number;
}>;

/** Measured CSS-pixel size of the React Flow host. */
export type ProjectContextCanvasSize = Readonly<{
  width: number;
  height: number;
}>;

/** React Flow viewport transform in screen pixels and graph zoom. */
export type ProjectContextViewport = Readonly<{
  x: number;
  y: number;
  zoom: number;
}>;

/** Axis-aligned graph-world bounds. */
export type ProjectContextBounds = Readonly<{
  x: number;
  y: number;
  width: number;
  height: number;
}>;

/** Captured authority at the start of one deferred resize correction. */
export type ProjectContextViewportResizeFence = Readonly<{
  fitGeneration: number;
  humanViewportGeneration: number;
  queryIdentity: string;
  resizeSequence: number;
  textScaleGeneration: number;
}>;

/** Content-free monotonic authority exposed by the graph test seam. */
export type ProjectContextViewportAuthorityState = Readonly<{
  authorityGeneration: number;
  authorityPending: boolean;
  humanViewportGeneration: number;
}>;

/** No external chrome reservation. */
export const EMPTY_PROJECT_CONTEXT_CANVAS_INSETS: ProjectContextCanvasInsets =
  Object.freeze({ top: 0, right: 0, bottom: 0, left: 0 });

function finiteNonNegative(value: number | undefined): number {
  return Number.isFinite(value) ? Math.max(0, value ?? 0) : 0;
}

/** Merge independently measured canvas chrome without allowing invalid space. */
export function mergeProjectContextCanvasInsets(
  ...insets: ReadonlyArray<Partial<ProjectContextCanvasInsets> | undefined>
): ProjectContextCanvasInsets {
  return insets.reduce<ProjectContextCanvasInsets>(
    (merged, next) => ({
      top: Math.max(merged.top, finiteNonNegative(next?.top)),
      right: Math.max(merged.right, finiteNonNegative(next?.right)),
      bottom: Math.max(merged.bottom, finiteNonNegative(next?.bottom)),
      left: Math.max(merged.left, finiteNonNegative(next?.left)),
    }),
    EMPTY_PROJECT_CONTEXT_CANVAS_INSETS,
  );
}

/** The unobscured screen-space rectangle available to graph content. */
export function projectContextSafeCanvasRect(
  size: ProjectContextCanvasSize,
  insets: ProjectContextCanvasInsets,
) {
  const width = Math.max(0, size.width - insets.left - insets.right);
  const height = Math.max(0, size.height - insets.top - insets.bottom);
  return { x: insets.left, y: insets.top, width, height };
}

/**
 * Calculate one React Flow viewport that fits bounds inside measured chrome.
 * Unlike padding-only fitting, the returned translation targets the safe rect.
 */
export function projectContextViewportForBounds({
  bounds,
  canvasSize,
  insets,
  maxZoom,
  minZoom,
  padding,
}: {
  bounds: ProjectContextBounds;
  canvasSize: ProjectContextCanvasSize;
  insets: ProjectContextCanvasInsets;
  maxZoom: number;
  minZoom: number;
  padding: number;
}): ProjectContextViewport | null {
  const safeRect = projectContextSafeCanvasRect(canvasSize, insets);
  if (
    safeRect.width <= 0 ||
    safeRect.height <= 0 ||
    bounds.width <= 0 ||
    bounds.height <= 0
  ) {
    return null;
  }
  const viewport = getViewportForBounds(
    bounds,
    safeRect.width,
    safeRect.height,
    minZoom,
    maxZoom,
    padding,
  );
  return {
    x: viewport.x + safeRect.x,
    y: viewport.y + safeRect.y,
    zoom: viewport.zoom,
  };
}

/** Canonical layout fitting does not depend on viewport-driven DOM visibility. */
export function projectContextCanonicalFitIsReady({
  canvasSize,
  chromeReady,
  fitSuspended,
}: {
  canvasSize: ProjectContextCanvasSize | null;
  chromeReady: boolean;
  fitSuspended: boolean;
}): boolean {
  return Boolean(
    chromeReady &&
      !fitSuspended &&
      canvasSize &&
      canvasSize.width > 0 &&
      canvasSize.height > 0,
  );
}

/** Preserve the graph-world point at canvas center across a host resize. */
export function recenterProjectContextViewportForResize({
  nextSize,
  previousSize,
  viewport,
}: {
  nextSize: ProjectContextCanvasSize;
  previousSize: ProjectContextCanvasSize;
  viewport: ProjectContextViewport;
}): ProjectContextViewport {
  if (
    previousSize.width <= 0 ||
    previousSize.height <= 0 ||
    nextSize.width <= 0 ||
    nextSize.height <= 0 ||
    viewport.zoom <= 0
  ) {
    return viewport;
  }
  const worldCenterX = (previousSize.width / 2 - viewport.x) / viewport.zoom;
  const worldCenterY = (previousSize.height / 2 - viewport.y) / viewport.zoom;
  return {
    x: nextSize.width / 2 - worldCenterX * viewport.zoom,
    y: nextSize.height / 2 - worldCenterY * viewport.zoom,
    zoom: viewport.zoom,
  };
}

/** Preserve the old canvas-center focal point while graph geometry rescales. */
export function recenterProjectContextViewportForTextScale({
  nextSize,
  previousSize,
  scaleRatio,
  viewport,
}: {
  nextSize: ProjectContextCanvasSize;
  previousSize: ProjectContextCanvasSize;
  scaleRatio: number;
  viewport: ProjectContextViewport;
}): ProjectContextViewport {
  if (
    previousSize.width <= 0 ||
    previousSize.height <= 0 ||
    nextSize.width <= 0 ||
    nextSize.height <= 0 ||
    viewport.zoom <= 0 ||
    !Number.isFinite(scaleRatio) ||
    scaleRatio <= 0
  ) {
    return viewport;
  }
  const previousWorldCenterX =
    (previousSize.width / 2 - viewport.x) / viewport.zoom;
  const previousWorldCenterY =
    (previousSize.height / 2 - viewport.y) / viewport.zoom;
  return {
    x: nextSize.width / 2 - previousWorldCenterX * scaleRatio * viewport.zoom,
    y: nextSize.height / 2 - previousWorldCenterY * scaleRatio * viewport.zoom,
    zoom: viewport.zoom,
  };
}

/** A deferred resize correction may only use the authority it captured. */
export function projectContextViewportResizeFenceIsCurrent(
  captured: ProjectContextViewportResizeFence,
  current: ProjectContextViewportResizeFence,
): boolean {
  return (
    captured.fitGeneration === current.fitGeneration &&
    captured.humanViewportGeneration === current.humanViewportGeneration &&
    captured.queryIdentity === current.queryIdentity &&
    captured.resizeSequence === current.resizeSequence &&
    captured.textScaleGeneration === current.textScaleGeneration
  );
}

/**
 * Promise completions are advisory: only the newest authority, chrome, and
 * text-scale submission may commit viewport-owned state.
 */
export function projectContextViewportOperationCanCommit({
  authority,
  chromeGeneration,
  currentAuthority,
  currentChromeGeneration,
  currentTextScaleGeneration,
  textScaleGeneration,
}: {
  authority: number;
  chromeGeneration?: number;
  currentAuthority: number;
  currentChromeGeneration?: number;
  currentTextScaleGeneration?: number;
  textScaleGeneration?: number;
}): boolean {
  return (
    authority === currentAuthority &&
    (chromeGeneration === undefined ||
      chromeGeneration === currentChromeGeneration) &&
    (textScaleGeneration === undefined ||
      textScaleGeneration === currentTextScaleGeneration)
  );
}

/** Bounds materialized at one text scale cannot be reused at another scale. */
export function projectContextFitBoundsMatchTextScale(
  capturedTextScaleGeneration: number,
  currentTextScaleGeneration: number,
): boolean {
  return capturedTextScaleGeneration === currentTextScaleGeneration;
}

/** Claim viewport authority, advancing the Human fence for direct input. */
export function beginProjectContextViewportAuthority(
  state: ProjectContextViewportAuthorityState,
  kind: "human" | "programmatic",
): ProjectContextViewportAuthorityState {
  return {
    authorityGeneration: state.authorityGeneration + 1,
    authorityPending: true,
    humanViewportGeneration:
      state.humanViewportGeneration + (kind === "human" ? 1 : 0),
  };
}

/** Close one operation only while it still owns current viewport authority. */
export function settleProjectContextViewportAuthority(
  state: ProjectContextViewportAuthorityState,
  authority: number,
  invalidate: boolean,
): ProjectContextViewportAuthorityState {
  if (state.authorityGeneration !== authority) return state;
  return {
    ...state,
    authorityGeneration: state.authorityGeneration + (invalidate ? 1 : 0),
    authorityPending: false,
  };
}

/** Bounded fallback for React Flow animation promises that may be interrupted. */
export function projectContextViewportOperationDeadlineMs(
  durationMs: number,
): number {
  const finiteDuration = Number.isFinite(durationMs)
    ? Math.max(0, durationMs)
    : 0;
  return Math.max(750, finiteDuration + 500);
}

/** Stable half-pixel geometry used by ResizeObserver generation fencing. */
export function quantizeProjectContextGeometry(value: number): number {
  return Math.round(value * 2) / 2;
}
