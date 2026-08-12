import * as React from "react";

import {
  beginProjectContextViewportAuthority,
  projectContextViewportOperationDeadlineMs,
  settleProjectContextViewportAuthority,
} from "@/features/project-context/projectContextViewport";

type ViewportAuthorityKind = "human" | "programmatic";

type TrackedViewportOperation = {
  authority: number;
  timeout: number;
  token: number;
};

export type TrackViewportOperationOptions = {
  authority: number;
  canCommit?: () => boolean;
  duration: number;
  onCommit?: () => void;
  onSettled: () => void;
  operation: Promise<boolean>;
};

export type TrackProjectContextViewportOperation = (
  options: TrackViewportOperationOptions,
) => void;

/** Own monotonic viewport authority and bound interrupted React Flow promises. */
export function useProjectContextViewportAuthority() {
  const authorityGeneration = React.useRef(0);
  const authorityPending = React.useRef(false);
  const humanViewportGeneration = React.useRef(0);
  const operationSequence = React.useRef(0);
  const activeOperation = React.useRef<TrackedViewportOperation | null>(null);
  const [snapshot, setSnapshot] = React.useState({
    authorityGeneration: 0,
    authorityPending: false,
    humanViewportGeneration: 0,
  });

  const clearActiveOperation = React.useCallback(() => {
    const active = activeOperation.current;
    if (active) window.clearTimeout(active.timeout);
    activeOperation.current = null;
  }, []);

  React.useEffect(() => clearActiveOperation, [clearActiveOperation]);

  const publishSnapshot = React.useCallback(() => {
    setSnapshot({
      authorityGeneration: authorityGeneration.current,
      authorityPending: authorityPending.current,
      humanViewportGeneration: humanViewportGeneration.current,
    });
  }, []);

  const beginAuthority = React.useCallback(
    (kind: ViewportAuthorityKind = "programmatic") => {
      clearActiveOperation();
      const next = beginProjectContextViewportAuthority(
        {
          authorityGeneration: authorityGeneration.current,
          authorityPending: authorityPending.current,
          humanViewportGeneration: humanViewportGeneration.current,
        },
        kind,
      );
      authorityGeneration.current = next.authorityGeneration;
      authorityPending.current = next.authorityPending;
      humanViewportGeneration.current = next.humanViewportGeneration;
      publishSnapshot();
      return authorityGeneration.current;
    },
    [clearActiveOperation, publishSnapshot],
  );

  const settleAuthority = React.useCallback(
    (authority: number) => {
      const current = {
        authorityGeneration: authorityGeneration.current,
        authorityPending: authorityPending.current,
        humanViewportGeneration: humanViewportGeneration.current,
      };
      const next = settleProjectContextViewportAuthority(
        current,
        authority,
        false,
      );
      if (next === current) return false;
      clearActiveOperation();
      authorityGeneration.current = next.authorityGeneration;
      authorityPending.current = next.authorityPending;
      publishSnapshot();
      return true;
    },
    [clearActiveOperation, publishSnapshot],
  );

  const invalidateAuthority = React.useCallback(
    (authority: number) => {
      const current = {
        authorityGeneration: authorityGeneration.current,
        authorityPending: authorityPending.current,
        humanViewportGeneration: humanViewportGeneration.current,
      };
      const next = settleProjectContextViewportAuthority(
        current,
        authority,
        true,
      );
      if (next === current) return false;
      clearActiveOperation();
      authorityGeneration.current = next.authorityGeneration;
      authorityPending.current = next.authorityPending;
      publishSnapshot();
      return true;
    },
    [clearActiveOperation, publishSnapshot],
  );

  const currentAuthority = React.useCallback(
    () => authorityGeneration.current,
    [],
  );
  const currentHumanViewportGeneration = React.useCallback(
    () => humanViewportGeneration.current,
    [],
  );

  const trackOperation = React.useCallback(
    ({
      authority,
      canCommit,
      duration,
      onCommit,
      onSettled,
      operation,
    }: TrackViewportOperationOptions) => {
      clearActiveOperation();
      const token = operationSequence.current + 1;
      operationSequence.current = token;
      const timeout = window.setTimeout(() => {
        const active = activeOperation.current;
        if (active?.token !== token || active.authority !== authority) return;
        activeOperation.current = null;
        if (invalidateAuthority(authority)) onSettled();
      }, projectContextViewportOperationDeadlineMs(duration));
      activeOperation.current = { authority, timeout, token };

      void operation
        .then(
          (completed) => {
            const active = activeOperation.current;
            if (
              !completed ||
              active?.token !== token ||
              active.authority !== authority ||
              authorityGeneration.current !== authority ||
              (canCommit && !canCommit())
            ) {
              return;
            }
            onCommit?.();
          },
          () => undefined,
        )
        .finally(() => {
          const active = activeOperation.current;
          if (active?.token !== token || active.authority !== authority) return;
          if (settleAuthority(authority)) onSettled();
        });
    },
    [clearActiveOperation, invalidateAuthority, settleAuthority],
  );

  const armHumanInteractionFallback = React.useCallback(
    (authority: number, onSettled: () => void) => {
      clearActiveOperation();
      const token = operationSequence.current + 1;
      operationSequence.current = token;
      const timeout = window.setTimeout(() => {
        const active = activeOperation.current;
        if (active?.token !== token || active.authority !== authority) return;
        activeOperation.current = null;
        if (settleAuthority(authority)) onSettled();
      }, projectContextViewportOperationDeadlineMs(220));
      activeOperation.current = { authority, timeout, token };
    },
    [clearActiveOperation, settleAuthority],
  );

  return {
    armHumanInteractionFallback,
    authorityPending,
    beginAuthority,
    currentAuthority,
    currentHumanViewportGeneration,
    humanViewportGeneration,
    invalidateAuthority,
    settleAuthority,
    snapshot,
    trackOperation,
  };
}
