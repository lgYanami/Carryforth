import assert from "node:assert/strict";
import test from "node:test";

import {
  beginProjectContextViewportAuthority,
  mergeProjectContextCanvasInsets,
  projectContextCanonicalFitIsReady,
  projectContextFitBoundsMatchTextScale,
  projectContextSafeCanvasRect,
  projectContextViewportOperationCanCommit,
  projectContextViewportOperationDeadlineMs,
  projectContextViewportForBounds,
  projectContextViewportResizeFenceIsCurrent,
  quantizeProjectContextGeometry,
  recenterProjectContextViewportForResize,
  recenterProjectContextViewportForTextScale,
  settleProjectContextViewportAuthority,
} from "./projectContextViewport.ts";

test("canvas insets merge independent chrome using the largest safe edge", () => {
  assert.deepEqual(
    mergeProjectContextCanvasInsets(
      { top: 72, right: 48 },
      { top: 56, right: 84, bottom: 52 },
      { left: -4, bottom: Number.NaN },
    ),
    { top: 72, right: 84, bottom: 52, left: 0 },
  );
});

test("safe canvas rect removes measured chrome from each edge", () => {
  assert.deepEqual(
    projectContextSafeCanvasRect(
      { width: 1_200, height: 800 },
      { top: 80, right: 64, bottom: 56, left: 16 },
    ),
    { x: 16, y: 80, width: 1_120, height: 664 },
  );
});

test("safe-area fit places graph bounds inside the unobscured rectangle", () => {
  const bounds = { x: 100, y: 200, width: 800, height: 400 };
  const canvasSize = { width: 1_200, height: 800 };
  const insets = { top: 96, right: 112, bottom: 72, left: 20 };
  const viewport = projectContextViewportForBounds({
    bounds,
    canvasSize,
    insets,
    minZoom: 0.12,
    maxZoom: 1.15,
    padding: 0.08,
  });
  assert.ok(viewport);
  const safe = projectContextSafeCanvasRect(canvasSize, insets);
  const transformed = {
    left: bounds.x * viewport.zoom + viewport.x,
    top: bounds.y * viewport.zoom + viewport.y,
    right: (bounds.x + bounds.width) * viewport.zoom + viewport.x,
    bottom: (bounds.y + bounds.height) * viewport.zoom + viewport.y,
  };
  assert.ok(transformed.left >= safe.x - 0.001);
  assert.ok(transformed.top >= safe.y - 0.001);
  assert.ok(transformed.right <= safe.x + safe.width + 0.001);
  assert.ok(transformed.bottom <= safe.y + safe.height + 0.001);
});

test("dense canonical bounds can fit below the legacy card zoom floor", () => {
  const bounds = { x: 0, y: 0, width: 9_508.25, height: 6_795.25 };
  const canvasSize = { width: 1_024, height: 700 };
  const insets = { top: 92, right: 212, bottom: 64, left: 12 };
  const viewport = projectContextViewportForBounds({
    bounds,
    canvasSize,
    insets,
    minZoom: 0.05,
    maxZoom: 1.15,
    padding: 0.08,
  });
  assert.ok(viewport);
  assert.ok(viewport.zoom < 0.12);
  const safe = projectContextSafeCanvasRect(canvasSize, insets);
  assert.ok(bounds.x * viewport.zoom + viewport.x >= safe.x - 0.001);
  assert.ok(bounds.y * viewport.zoom + viewport.y >= safe.y - 0.001);
  assert.ok(
    (bounds.x + bounds.width) * viewport.zoom + viewport.x <=
      safe.x + safe.width + 0.001,
  );
  assert.ok(
    (bounds.y + bounds.height) * viewport.zoom + viewport.y <=
      safe.y + safe.height + 0.001,
  );
});

test("safe-area fit rejects zero-sized canvas and bounds", () => {
  const common = {
    insets: { top: 0, right: 0, bottom: 0, left: 0 },
    minZoom: 0.12,
    maxZoom: 1.15,
    padding: 0.08,
  };
  assert.equal(
    projectContextViewportForBounds({
      ...common,
      bounds: { x: 0, y: 0, width: 10, height: 10 },
      canvasSize: { width: 0, height: 100 },
    }),
    null,
  );
  assert.equal(
    projectContextViewportForBounds({
      ...common,
      bounds: { x: 0, y: 0, width: 0, height: 10 },
      canvasSize: { width: 100, height: 100 },
    }),
    null,
  );
});

test("canonical fit readiness depends on stable chrome and canvas, not visible DOM nodes", () => {
  assert.equal(
    projectContextCanonicalFitIsReady({
      canvasSize: { width: 1_200, height: 800 },
      chromeReady: true,
      fitSuspended: false,
    }),
    true,
  );
  assert.equal(
    projectContextCanonicalFitIsReady({
      canvasSize: { width: 1_200, height: 800 },
      chromeReady: false,
      fitSuspended: false,
    }),
    false,
  );
  assert.equal(
    projectContextCanonicalFitIsReady({
      canvasSize: { width: 1_200, height: 800 },
      chromeReady: true,
      fitSuspended: true,
    }),
    false,
  );
});

test("resize recentering preserves zoom and world point at canvas center", () => {
  const viewport = { x: -320, y: 48, zoom: 0.8 };
  const previousSize = { width: 1_200, height: 800 };
  const nextSize = { width: 760, height: 800 };
  const recentered = recenterProjectContextViewportForResize({
    viewport,
    previousSize,
    nextSize,
  });
  const before = {
    x: (previousSize.width / 2 - viewport.x) / viewport.zoom,
    y: (previousSize.height / 2 - viewport.y) / viewport.zoom,
  };
  const after = {
    x: (nextSize.width / 2 - recentered.x) / recentered.zoom,
    y: (nextSize.height / 2 - recentered.y) / recentered.zoom,
  };
  assert.equal(recentered.zoom, viewport.zoom);
  assert.ok(Math.abs(before.x - after.x) < 1e-9);
  assert.ok(Math.abs(before.y - after.y) < 1e-9);
});

test("text scaling preserves the old canvas-center focal point across docked-to-Drawer resize", () => {
  const viewport = { x: -340, y: 75, zoom: 0.7 };
  const previousSize = { width: 860, height: 720 };
  const nextSize = { width: 1_180, height: 720 };
  const scaleRatio = 1.5;
  const recentered = recenterProjectContextViewportForTextScale({
    nextSize,
    previousSize,
    scaleRatio,
    viewport,
  });
  const previousWorldCenter = {
    x: (previousSize.width / 2 - viewport.x) / viewport.zoom,
    y: (previousSize.height / 2 - viewport.y) / viewport.zoom,
  };
  const nextWorldCenter = {
    x: (nextSize.width / 2 - recentered.x) / recentered.zoom,
    y: (nextSize.height / 2 - recentered.y) / recentered.zoom,
  };
  assert.equal(recentered.zoom, viewport.zoom);
  assert.ok(
    Math.abs(nextWorldCenter.x - previousWorldCenter.x * scaleRatio) < 1e-9,
  );
  assert.ok(
    Math.abs(nextWorldCenter.y - previousWorldCenter.y * scaleRatio) < 1e-9,
  );
});

test("measurement geometry quantizes to stable half pixels", () => {
  assert.equal(quantizeProjectContextGeometry(48.24), 48);
  assert.equal(quantizeProjectContextGeometry(48.25), 48.5);
  assert.equal(quantizeProjectContextGeometry(48.74), 48.5);
  assert.equal(quantizeProjectContextGeometry(48.75), 49);
});

test("resize correction rejects an anchor captured before Human viewport input", () => {
  const captured = {
    fitGeneration: 4,
    humanViewportGeneration: 2,
    queryIdentity: "revision:30",
    resizeSequence: 8,
    textScaleGeneration: 1,
  };
  assert.equal(
    projectContextViewportResizeFenceIsCurrent(captured, captured),
    true,
  );
  assert.equal(
    projectContextViewportResizeFenceIsCurrent(captured, {
      ...captured,
      humanViewportGeneration: 3,
    }),
    false,
  );
});

test("resize correction rejects stale fit, query, text-scale, and sequence fences", () => {
  const captured = {
    fitGeneration: 4,
    humanViewportGeneration: 2,
    queryIdentity: "revision:30",
    resizeSequence: 8,
    textScaleGeneration: 1,
  };
  for (const current of [
    { ...captured, fitGeneration: 5 },
    { ...captured, queryIdentity: "revision:31" },
    { ...captured, resizeSequence: 9 },
    { ...captured, textScaleGeneration: 2 },
  ]) {
    assert.equal(
      projectContextViewportResizeFenceIsCurrent(captured, current),
      false,
    );
  }
});

test("stale animation completions cannot commit newer Human or chrome authority", () => {
  assert.equal(
    projectContextViewportOperationCanCommit({
      authority: 7,
      chromeGeneration: 12,
      currentAuthority: 7,
      currentChromeGeneration: 12,
    }),
    true,
  );
  assert.equal(
    projectContextViewportOperationCanCommit({
      authority: 7,
      chromeGeneration: 12,
      currentAuthority: 8,
      currentChromeGeneration: 12,
    }),
    false,
  );
  assert.equal(
    projectContextViewportOperationCanCommit({
      authority: 7,
      chromeGeneration: 12,
      currentAuthority: 7,
      currentChromeGeneration: 13,
    }),
    false,
  );
});

test("text-scale changes invalidate captured Fit bounds and late completions", () => {
  assert.equal(projectContextFitBoundsMatchTextScale(3, 3), true);
  assert.equal(projectContextFitBoundsMatchTextScale(3, 4), false);
  assert.equal(
    projectContextViewportOperationCanCommit({
      authority: 7,
      chromeGeneration: 12,
      currentAuthority: 7,
      currentChromeGeneration: 12,
      currentTextScaleGeneration: 4,
      textScaleGeneration: 3,
    }),
    false,
  );
});

test("viewport operation fallback always exceeds its requested animation", () => {
  assert.equal(projectContextViewportOperationDeadlineMs(0), 750);
  assert.equal(projectContextViewportOperationDeadlineMs(220), 750);
  assert.equal(projectContextViewportOperationDeadlineMs(800), 1_300);
  assert.equal(projectContextViewportOperationDeadlineMs(Number.NaN), 750);
});

test("Human input advances both viewport authority and its resize fence", () => {
  const fit = beginProjectContextViewportAuthority(
    {
      authorityGeneration: 4,
      authorityPending: false,
      humanViewportGeneration: 2,
    },
    "programmatic",
  );
  assert.deepEqual(fit, {
    authorityGeneration: 5,
    authorityPending: true,
    humanViewportGeneration: 2,
  });

  const human = beginProjectContextViewportAuthority(fit, "human");
  assert.deepEqual(human, {
    authorityGeneration: 6,
    authorityPending: true,
    humanViewportGeneration: 3,
  });
  assert.equal(settleProjectContextViewportAuthority(human, 5, false), human);
  assert.deepEqual(settleProjectContextViewportAuthority(human, 6, false), {
    ...human,
    authorityPending: false,
  });
});

test("timed-out authority closes pending and invalidates its late promise", () => {
  const pending = beginProjectContextViewportAuthority(
    {
      authorityGeneration: 8,
      authorityPending: false,
      humanViewportGeneration: 3,
    },
    "programmatic",
  );
  const expired = settleProjectContextViewportAuthority(
    pending,
    pending.authorityGeneration,
    true,
  );
  assert.deepEqual(expired, {
    authorityGeneration: 10,
    authorityPending: false,
    humanViewportGeneration: 3,
  });
  assert.equal(
    projectContextViewportOperationCanCommit({
      authority: pending.authorityGeneration,
      currentAuthority: expired.authorityGeneration,
    }),
    false,
  );
});
