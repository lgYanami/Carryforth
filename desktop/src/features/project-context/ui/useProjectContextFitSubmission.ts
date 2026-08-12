import type { Viewport } from "@xyflow/react";
import * as React from "react";

import type {
  ProjectContextBounds,
  ProjectContextLayout,
} from "@/features/project-context/layout";
import {
  type ProjectContextCanvasInsets,
  projectContextCanonicalFitIsReady,
  projectContextFitBoundsMatchTextScale,
  projectContextViewportOperationCanCommit,
  projectContextViewportForBounds,
} from "@/features/project-context/projectContextViewport";
import type { ProjectContextGraphTarget } from "@/features/project-context/presentation";
import { sameProjectContextGraphTarget } from "@/features/project-context/ui/projectContextGraphInteraction";
import type { TrackProjectContextViewportOperation } from "@/features/project-context/ui/useProjectContextViewportAuthority";

export type ProjectContextFitRequest = {
  authority: number;
  bounds: ProjectContextBounds;
  completion:
    | { kind: "query"; key: string }
    | { kind: "semantic"; key: string }
    | { kind: "semantic-request"; key: number }
    | { kind: "focus-request"; key: number; target: ProjectContextGraphTarget }
    | { kind: "manual" };
  duration: number;
  humanViewportGeneration: number;
  maxZoom: number;
  padding: number;
  queryIdentity: string;
  semanticGeneration?: string;
  textScaleGeneration: number;
};

export type ProjectContextFitQueueRequest = Omit<
  ProjectContextFitRequest,
  | "authority"
  | "humanViewportGeneration"
  | "queryIdentity"
  | "textScaleGeneration"
>;

type ChromeMeasurement = Readonly<{
  generation: number;
  insets: ProjectContextCanvasInsets;
  ready: boolean;
}>;

function rebuildFitForCurrentTextScale(
  request: ProjectContextFitRequest,
  layout: ProjectContextLayout,
  semanticBounds: ProjectContextBounds | null,
  semanticGeneration: string,
): ProjectContextFitQueueRequest | null {
  let bounds: ProjectContextBounds | null;
  switch (request.completion.kind) {
    case "query":
      bounds = layout.bounds;
      break;
    case "semantic":
    case "semantic-request":
      bounds = semanticBounds;
      break;
    case "focus-request": {
      const nodeId =
        request.completion.target.kind === "coordinate"
          ? `coordinate:${request.completion.target.key}`
          : `edge-hub:${request.completion.target.key}`;
      bounds = layout.nodes.find((node) => node.id === nodeId) ?? null;
      break;
    }
    case "manual":
      return null;
  }
  if (!bounds) return null;
  return {
    bounds,
    completion: request.completion,
    duration: request.duration,
    maxZoom: request.maxZoom,
    padding: request.padding,
    ...(request.semanticGeneration === undefined ? {} : { semanticGeneration }),
  };
}

/** Submit only the latest safe-area Fit and close every async exit path. */
export function useProjectContextFitSubmission({
  authoritySnapshot,
  currentAuthority,
  currentHumanViewportGeneration,
  fitGeneration,
  fitSuspended,
  invalidateAuthority,
  layout,
  measurement,
  measurementGeneration,
  minZoom,
  onCanceled,
  onCommitted,
  pendingFit,
  queryIdentity,
  queueFit,
  resetResizeBaseline,
  rootRef,
  selection,
  semanticBounds,
  semanticGeneration,
  setPendingFit,
  setViewport,
  submittedFit,
  textScaleGeneration,
  trackOperation,
}: {
  authoritySnapshot: Readonly<{
    authorityGeneration: number;
    humanViewportGeneration: number;
  }>;
  currentAuthority: () => number;
  currentHumanViewportGeneration: () => number;
  fitGeneration: React.RefObject<number>;
  fitSuspended: boolean;
  invalidateAuthority: (authority: number) => boolean;
  layout: ProjectContextLayout;
  measurement: ChromeMeasurement;
  measurementGeneration: React.RefObject<number>;
  minZoom: number;
  onCanceled: (
    request: ProjectContextFitRequest,
    preserveQueuedRequest: boolean,
  ) => void;
  onCommitted: (request: ProjectContextFitRequest) => void;
  pendingFit: ProjectContextFitRequest | null;
  queryIdentity: React.RefObject<string>;
  queueFit: (request: ProjectContextFitQueueRequest) => void;
  resetResizeBaseline: () => void;
  rootRef: React.RefObject<HTMLElement | null>;
  selection: ProjectContextGraphTarget | null;
  semanticBounds: ProjectContextBounds | null;
  semanticGeneration: React.RefObject<string>;
  setPendingFit: React.Dispatch<
    React.SetStateAction<ProjectContextFitRequest | null>
  >;
  setViewport: (
    viewport: Viewport,
    options?: { duration?: number },
  ) => Promise<boolean>;
  submittedFit: React.RefObject<{
    authority: number;
    chromeGeneration: number;
  } | null>;
  textScaleGeneration: React.RefObject<number>;
  trackOperation: TrackProjectContextViewportOperation;
}) {
  React.useEffect(() => {
    if (!pendingFit) return;
    const staleAuthority =
      pendingFit.authority !== authoritySnapshot.authorityGeneration ||
      pendingFit.authority !== currentAuthority();
    const staleHumanAuthority =
      pendingFit.humanViewportGeneration !==
        authoritySnapshot.humanViewportGeneration ||
      pendingFit.humanViewportGeneration !== currentHumanViewportGeneration();
    const staleTextScale = !projectContextFitBoundsMatchTextScale(
      pendingFit.textScaleGeneration,
      textScaleGeneration.current,
    );
    const staleIdentity =
      pendingFit.queryIdentity !== queryIdentity.current ||
      (pendingFit.semanticGeneration !== undefined &&
        pendingFit.semanticGeneration !== semanticGeneration.current) ||
      (pendingFit.completion.kind === "focus-request" &&
        !sameProjectContextGraphTarget(
          selection,
          pendingFit.completion.target,
        ));
    if (staleAuthority || staleIdentity || staleTextScale) {
      if (staleTextScale && !staleHumanAuthority && !staleIdentity) {
        const replacement = rebuildFitForCurrentTextScale(
          pendingFit,
          layout,
          semanticBounds,
          semanticGeneration.current,
        );
        if (replacement) {
          queueFit(replacement);
          return;
        }
      }
      if (
        staleAuthority &&
        !staleHumanAuthority &&
        !staleIdentity &&
        !staleTextScale
      ) {
        const {
          authority: _authority,
          humanViewportGeneration: _humanViewportGeneration,
          queryIdentity: _query,
          textScaleGeneration: _textScaleGeneration,
          ...request
        } = pendingFit;
        queueFit(request);
        return;
      }
      onCanceled(pendingFit, staleHumanAuthority);
      if (invalidateAuthority(pendingFit.authority)) {
        fitGeneration.current += 1;
        resetResizeBaseline();
      }
      setPendingFit((current) =>
        current?.authority === pendingFit.authority ? null : current,
      );
      return;
    }

    const root = rootRef.current;
    const rootSize = root
      ? { width: root.clientWidth, height: root.clientHeight }
      : null;
    if (
      !projectContextCanonicalFitIsReady({
        canvasSize: rootSize,
        chromeReady: measurement.ready,
        fitSuspended,
      }) ||
      (pendingFit.completion.kind === "semantic" &&
        pendingFit.completion.key !== semanticGeneration.current)
    ) {
      return;
    }
    if (!rootSize) return;

    const submission = submittedFit.current;
    if (submission?.authority === pendingFit.authority) {
      if (submission.chromeGeneration === measurement.generation) return;
      const {
        authority: _authority,
        humanViewportGeneration: _humanViewportGeneration,
        queryIdentity: _query,
        textScaleGeneration: _textScaleGeneration,
        ...request
      } = pendingFit;
      queueFit(request);
      return;
    }
    const viewport = projectContextViewportForBounds({
      bounds: pendingFit.bounds,
      canvasSize: rootSize,
      insets: measurement.insets,
      maxZoom: pendingFit.maxZoom,
      minZoom,
      padding: pendingFit.padding,
    });
    if (!viewport) return;

    const chromeGeneration = measurement.generation;
    submittedFit.current = {
      authority: pendingFit.authority,
      chromeGeneration,
    };
    trackOperation({
      authority: pendingFit.authority,
      canCommit: () =>
        projectContextViewportOperationCanCommit({
          authority: pendingFit.authority,
          chromeGeneration,
          currentAuthority: currentAuthority(),
          currentChromeGeneration: measurementGeneration.current,
          currentTextScaleGeneration: textScaleGeneration.current,
          textScaleGeneration: pendingFit.textScaleGeneration,
        }) &&
        submittedFit.current?.authority === pendingFit.authority &&
        submittedFit.current.chromeGeneration === chromeGeneration,
      duration: pendingFit.duration,
      onCommit: () => onCommitted(pendingFit),
      onSettled: () => {
        if (
          submittedFit.current?.authority === pendingFit.authority &&
          submittedFit.current.chromeGeneration === chromeGeneration
        ) {
          submittedFit.current = null;
        }
        resetResizeBaseline();
        setPendingFit((current) =>
          current?.authority === pendingFit.authority ? null : current,
        );
      },
      operation: setViewport(viewport, { duration: pendingFit.duration }),
    });
  }, [
    authoritySnapshot,
    currentAuthority,
    currentHumanViewportGeneration,
    fitGeneration,
    fitSuspended,
    invalidateAuthority,
    layout,
    measurement,
    measurementGeneration,
    minZoom,
    onCanceled,
    onCommitted,
    pendingFit,
    queryIdentity,
    queueFit,
    resetResizeBaseline,
    rootRef,
    selection,
    semanticBounds,
    semanticGeneration,
    setPendingFit,
    setViewport,
    submittedFit,
    textScaleGeneration,
    trackOperation,
  ]);
}
