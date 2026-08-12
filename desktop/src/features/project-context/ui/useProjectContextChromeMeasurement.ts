import * as React from "react";

import {
  mergeProjectContextCanvasInsets,
  type ProjectContextCanvasInsets,
  quantizeProjectContextGeometry,
} from "@/features/project-context/projectContextViewport";
import type { ProjectContextChromeContributor } from "@/features/project-context/ui/ProjectContextCanvasHud";

const CHROME_GAP_PX = 12;

type ProjectContextChromeMeasurement = {
  generation: number;
  insets: ProjectContextCanvasInsets;
  ready: boolean;
};

type ChromeSample = {
  insets: ProjectContextCanvasInsets;
  signature: string;
};

function quantizedRect(element: Element) {
  const rect = element.getBoundingClientRect();
  return {
    top: quantizeProjectContextGeometry(rect.top),
    right: quantizeProjectContextGeometry(rect.right),
    bottom: quantizeProjectContextGeometry(rect.bottom),
    left: quantizeProjectContextGeometry(rect.left),
    width: quantizeProjectContextGeometry(rect.width),
    height: quantizeProjectContextGeometry(rect.height),
  };
}

/** Measure every visible HUD contributor and wait for two stable frames. */
export function useProjectContextChromeMeasurement({
  expectedContributors,
  externalInsets,
  rootRef,
}: {
  expectedContributors: readonly ProjectContextChromeContributor[];
  externalInsets: ProjectContextCanvasInsets;
  rootRef: React.RefObject<HTMLElement | null>;
}) {
  const contributors = React.useRef(
    new Map<ProjectContextChromeContributor, HTMLDivElement>(),
  );
  const callbacks = React.useRef(
    new Map<
      ProjectContextChromeContributor,
      (element: HTMLDivElement | null) => void
    >(),
  );
  const [elementsVersion, setElementsVersion] = React.useState(0);
  const [measurement, setMeasurement] =
    React.useState<ProjectContextChromeMeasurement>({
      generation: 0,
      insets: mergeProjectContextCanvasInsets(
        { left: CHROME_GAP_PX },
        externalInsets,
      ),
      ready: false,
    });
  const generation = React.useRef(0);
  const animationFrames = React.useRef<number[]>([]);
  const expectedRef = React.useRef(expectedContributors);
  const externalRef = React.useRef(externalInsets);
  expectedRef.current = expectedContributors;
  externalRef.current = externalInsets;

  const sample = React.useCallback((): ChromeSample | null => {
    const root = rootRef.current;
    if (!root) return null;
    const rootRect = quantizedRect(root);
    if (rootRect.width <= 0 || rootRect.height <= 0) return null;
    const rects = new Map<
      ProjectContextChromeContributor,
      ReturnType<typeof quantizedRect>
    >();
    for (const contributor of expectedRef.current) {
      const element = contributors.current.get(contributor);
      if (!element) return null;
      rects.set(contributor, quantizedRect(element));
    }
    const summary = rects.get("summary");
    const selection = rects.get("selection");
    const controls = rects.get("controls");
    const guidance = rects.get("guidance");
    const measuredInsets = {
      top:
        Math.max(
          summary ? summary.bottom - rootRect.top : 0,
          selection ? selection.bottom - rootRect.top : 0,
        ) + CHROME_GAP_PX,
      right: (controls ? rootRect.right - controls.left : 0) + CHROME_GAP_PX,
      bottom:
        Math.max(
          controls ? rootRect.bottom - controls.top : 0,
          guidance ? rootRect.bottom - guidance.top : 0,
        ) + CHROME_GAP_PX,
      left: CHROME_GAP_PX,
    };
    const insets = mergeProjectContextCanvasInsets(
      measuredInsets,
      externalRef.current,
    );
    const signature = JSON.stringify({
      expected: expectedRef.current,
      external: externalRef.current,
      insets,
      rects: [...rects],
      rootRect,
    });
    return { insets, signature };
  }, [rootRef]);

  const scheduleStability = React.useRef<
    ((nextGeneration: number) => void) | null
  >(null);
  const invalidate = React.useCallback(() => {
    for (const frame of animationFrames.current) cancelAnimationFrame(frame);
    animationFrames.current = [];
    const nextGeneration = generation.current + 1;
    generation.current = nextGeneration;
    setMeasurement({
      generation: nextGeneration,
      insets:
        sample()?.insets ??
        mergeProjectContextCanvasInsets(
          { left: CHROME_GAP_PX },
          externalRef.current,
        ),
      ready: false,
    });
    scheduleStability.current?.(nextGeneration);
  }, [sample]);

  scheduleStability.current = (nextGeneration) => {
    const firstFrame = requestAnimationFrame(() => {
      const first = sample();
      if (!first || generation.current !== nextGeneration) return;
      const secondFrame = requestAnimationFrame(() => {
        const second = sample();
        if (generation.current !== nextGeneration) return;
        if (!second || first.signature !== second.signature) {
          invalidate();
          return;
        }
        animationFrames.current = [];
        setMeasurement({
          generation: nextGeneration,
          insets: second.insets,
          ready: true,
        });
      });
      animationFrames.current.push(secondFrame);
    });
    animationFrames.current.push(firstFrame);
  };

  const registerChromeContributor = React.useCallback(
    (contributor: ProjectContextChromeContributor) => {
      const existing = callbacks.current.get(contributor);
      if (existing) return existing;
      const callback = (element: HTMLDivElement | null) => {
        const previous = contributors.current.get(contributor);
        if (previous === element) return;
        if (element) contributors.current.set(contributor, element);
        else contributors.current.delete(contributor);
        setElementsVersion((current) => current + 1);
      };
      callbacks.current.set(contributor, callback);
      return callback;
    },
    [],
  );

  const externalKey = `${externalInsets.top}:${externalInsets.right}:${externalInsets.bottom}:${externalInsets.left}`;
  React.useLayoutEffect(() => {
    void elementsVersion;
    void externalKey;
    const root = rootRef.current;
    if (!root) return;
    let previousSignature = sample()?.signature;
    const observer = new ResizeObserver(() => {
      const nextSignature = sample()?.signature;
      if (nextSignature === previousSignature) return;
      previousSignature = nextSignature;
      invalidate();
    });
    observer.observe(root);
    for (const contributor of expectedContributors) {
      const element = contributors.current.get(contributor);
      if (element) observer.observe(element);
    }
    invalidate();
    return () => observer.disconnect();
  }, [
    elementsVersion,
    externalKey,
    expectedContributors,
    invalidate,
    rootRef,
    sample,
  ]);

  React.useEffect(
    () => () => {
      for (const frame of animationFrames.current) cancelAnimationFrame(frame);
    },
    [],
  );

  return { measurement, registerChromeContributor };
}
