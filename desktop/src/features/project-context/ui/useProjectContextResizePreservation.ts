import type { Viewport } from "@xyflow/react";
import * as React from "react";

import {
  type ProjectContextCanvasSize,
  type ProjectContextViewportResizeFence,
  projectContextViewportResizeFenceIsCurrent,
  quantizeProjectContextGeometry,
  recenterProjectContextViewportForResize,
} from "@/features/project-context/projectContextViewport";

function canvasSize(root: HTMLElement | null): ProjectContextCanvasSize | null {
  if (!root || root.clientWidth <= 0 || root.clientHeight <= 0) return null;
  return {
    width: quantizeProjectContextGeometry(root.clientWidth),
    height: quantizeProjectContextGeometry(root.clientHeight),
  };
}

/** Keep the same graph-world point at canvas center through host resizes. */
export function useProjectContextResizePreservation({
  authorityPending,
  fitGeneration,
  getViewport,
  humanViewportGeneration,
  queryIdentity,
  rootRef,
  setViewport,
  textScaleGeneration,
}: {
  authorityPending: React.RefObject<boolean>;
  fitGeneration: React.RefObject<number>;
  getViewport: () => Viewport;
  humanViewportGeneration: React.RefObject<number>;
  queryIdentity: React.RefObject<string>;
  rootRef: React.RefObject<HTMLElement | null>;
  setViewport: (
    viewport: Viewport,
    options?: { duration?: number },
  ) => Promise<boolean>;
  textScaleGeneration: React.RefObject<number>;
}) {
  const [correctionCount, setCorrectionCount] = React.useState(0);
  const baseline = React.useRef<ProjectContextCanvasSize | null>(null);
  const anchor = React.useRef<{
    fence: ProjectContextViewportResizeFence;
    previousSize: ProjectContextCanvasSize;
    viewport: Viewport;
  } | null>(null);
  const sequence = React.useRef(0);
  const animationFrames = React.useRef<number[]>([]);

  const resetBaseline = React.useCallback(() => {
    for (const frame of animationFrames.current) cancelAnimationFrame(frame);
    animationFrames.current = [];
    anchor.current = null;
    baseline.current = canvasSize(rootRef.current);
  }, [rootRef]);
  const getBaselineSize = React.useCallback(() => baseline.current, []);

  React.useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    baseline.current = canvasSize(root);
    const observer = new ResizeObserver(() => {
      const nextSize = canvasSize(root);
      const previousSize = baseline.current;
      if (!nextSize || !previousSize) {
        baseline.current = nextSize;
        return;
      }
      if (
        nextSize.width === previousSize.width &&
        nextSize.height === previousSize.height
      ) {
        return;
      }
      if (authorityPending.current) {
        // The active programmatic owner will reset this baseline when it
        // settles. Until then, keep the pre-resize canvas size available to a
        // same-commit text-scale focal correction.
        anchor.current = null;
        return;
      }
      for (const frame of animationFrames.current) cancelAnimationFrame(frame);
      animationFrames.current = [];
      const nextSequence = sequence.current + 1;
      sequence.current = nextSequence;
      const fence: ProjectContextViewportResizeFence = {
        fitGeneration: fitGeneration.current,
        humanViewportGeneration: humanViewportGeneration.current,
        queryIdentity: queryIdentity.current,
        resizeSequence: nextSequence,
        textScaleGeneration: textScaleGeneration.current,
      };
      anchor.current ??= {
        fence,
        previousSize,
        viewport: getViewport(),
      };
      anchor.current.fence = fence;
      const firstFrame = requestAnimationFrame(() => {
        const firstSize = canvasSize(root);
        const resizeAnchor = anchor.current;
        if (
          !firstSize ||
          !resizeAnchor ||
          authorityPending.current ||
          !projectContextViewportResizeFenceIsCurrent(resizeAnchor.fence, {
            fitGeneration: fitGeneration.current,
            humanViewportGeneration: humanViewportGeneration.current,
            queryIdentity: queryIdentity.current,
            resizeSequence: sequence.current,
            textScaleGeneration: textScaleGeneration.current,
          })
        ) {
          return;
        }
        const secondFrame = requestAnimationFrame(() => {
          const settledSize = canvasSize(root);
          const settledAnchor = anchor.current;
          if (
            !settledSize ||
            !settledAnchor ||
            nextSequence !== sequence.current ||
            firstSize.width !== settledSize.width ||
            firstSize.height !== settledSize.height ||
            authorityPending.current ||
            !projectContextViewportResizeFenceIsCurrent(settledAnchor.fence, {
              fitGeneration: fitGeneration.current,
              humanViewportGeneration: humanViewportGeneration.current,
              queryIdentity: queryIdentity.current,
              resizeSequence: sequence.current,
              textScaleGeneration: textScaleGeneration.current,
            })
          ) {
            return;
          }
          const viewport = recenterProjectContextViewportForResize({
            nextSize: settledSize,
            previousSize: settledAnchor.previousSize,
            viewport: settledAnchor.viewport,
          });
          baseline.current = settledSize;
          anchor.current = null;
          animationFrames.current = [];
          void setViewport(viewport, { duration: 0 }).then(
            (completed) => {
              if (
                completed &&
                projectContextViewportResizeFenceIsCurrent(
                  settledAnchor.fence,
                  {
                    fitGeneration: fitGeneration.current,
                    humanViewportGeneration: humanViewportGeneration.current,
                    queryIdentity: queryIdentity.current,
                    resizeSequence: sequence.current,
                    textScaleGeneration: textScaleGeneration.current,
                  },
                )
              ) {
                setCorrectionCount((current) => current + 1);
              }
            },
            () => undefined,
          );
        });
        animationFrames.current.push(secondFrame);
      });
      animationFrames.current.push(firstFrame);
    });
    observer.observe(root);
    return () => {
      observer.disconnect();
      for (const frame of animationFrames.current) cancelAnimationFrame(frame);
    };
  }, [
    authorityPending,
    fitGeneration,
    getViewport,
    humanViewportGeneration,
    queryIdentity,
    rootRef,
    setViewport,
    textScaleGeneration,
  ]);

  return { correctionCount, getBaselineSize, resetBaseline };
}
