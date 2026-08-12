import * as React from "react";

import type { useProjectContextViewportAuthority } from "@/features/project-context/ui/useProjectContextViewportAuthority";

type ViewportAuthority = ReturnType<typeof useProjectContextViewportAuthority>;

/** Let direct Human pan/zoom interrupt every older viewport operation. */
export function useProjectContextHumanViewportAuthority({
  armHumanInteractionFallback,
  beginAuthority,
  cancelPendingViewportOperation,
  duration,
  resetResizeBaseline,
  settleAuthority,
  trackOperation,
}: {
  armHumanInteractionFallback: ViewportAuthority["armHumanInteractionFallback"];
  beginAuthority: ViewportAuthority["beginAuthority"];
  cancelPendingViewportOperation: () => void;
  duration: number;
  resetResizeBaseline: () => void;
  settleAuthority: ViewportAuthority["settleAuthority"];
  trackOperation: ViewportAuthority["trackOperation"];
}) {
  const activeGestureAuthority = React.useRef<number | null>(null);
  const armGestureFallback = React.useCallback(
    (authority: number) => {
      armHumanInteractionFallback(authority, () => {
        if (activeGestureAuthority.current === authority) {
          activeGestureAuthority.current = null;
        }
        resetResizeBaseline();
      });
    },
    [armHumanInteractionFallback, resetResizeBaseline],
  );
  const claimAuthority = React.useCallback(() => {
    activeGestureAuthority.current = null;
    const authority = beginAuthority("human");
    cancelPendingViewportOperation();
    return authority;
  }, [beginAuthority, cancelPendingViewportOperation]);
  const beginGesture = React.useCallback(() => {
    const authority = claimAuthority();
    activeGestureAuthority.current = authority;
    armGestureFallback(authority);
  }, [armGestureFallback, claimAuthority]);
  const continueGesture = React.useCallback(() => {
    const authority = activeGestureAuthority.current;
    if (authority === null) {
      beginGesture();
      return;
    }
    armGestureFallback(authority);
  }, [armGestureFallback, beginGesture]);
  const endGesture = React.useCallback(() => {
    const authority = activeGestureAuthority.current;
    activeGestureAuthority.current = null;
    if (authority !== null && settleAuthority(authority)) {
      resetResizeBaseline();
    }
  }, [resetResizeBaseline, settleAuthority]);
  const runCommand = React.useCallback(
    (operation: () => Promise<boolean>) => {
      const authority = claimAuthority();
      trackOperation({
        authority,
        duration,
        onSettled: resetResizeBaseline,
        operation: operation(),
      });
    },
    [claimAuthority, duration, resetResizeBaseline, trackOperation],
  );

  return { beginGesture, continueGesture, endGesture, runCommand };
}
