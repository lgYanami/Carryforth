import * as React from "react";
import { flushSync } from "react-dom";

import {
  setThreadViewMode,
  type ThreadViewMode,
} from "@/features/channels/lib/threadViewModePreference";

export function findTopVisibleThreadMessageId(
  body: HTMLElement | null,
): string | null {
  if (!body) return null;

  const bodyTop = body.getBoundingClientRect().top;
  const visibleReply = Array.from(
    body.querySelectorAll<HTMLElement>("[data-message-id]"),
  ).find((row) => row.getBoundingClientRect().bottom > bodyTop);
  return visibleReply?.dataset.messageId ?? null;
}

export function getResolvedThreadTargets({
  externalTargetId,
  layoutTargetId,
}: {
  externalTargetId: string | null;
  layoutTargetId: string | null;
}) {
  return {
    resolveExternal:
      layoutTargetId === null || layoutTargetId === externalTargetId,
    resolveLayout: layoutTargetId !== null,
  };
}

type ThreadViewModeSwitchOptions = {
  externalScrollTargetId: string | null;
  onExternalTargetResolved: () => void;
  onModeChange?: (mode: ThreadViewMode) => void;
  viewMode: ThreadViewMode;
};

type LayoutScrollTarget = {
  messageId: string;
  viewMode: ThreadViewMode;
};

/** Preserves the reply being read while the thread changes presentation. */
export function useThreadViewModeSwitch({
  externalScrollTargetId,
  onExternalTargetResolved,
  onModeChange,
  viewMode,
}: ThreadViewModeSwitchOptions) {
  const [layoutScrollTarget, setLayoutScrollTarget] =
    React.useState<LayoutScrollTarget | null>(null);
  const layoutScrollTargetId =
    layoutScrollTarget?.viewMode === viewMode
      ? layoutScrollTarget.messageId
      : null;

  const changeThreadViewMode = React.useCallback(
    (mode: ThreadViewMode, restoreFocus: boolean) => {
      const body = document.querySelector<HTMLElement>(
        '[data-testid="message-thread-body"]',
      );
      const anchorId = findTopVisibleThreadMessageId(body);

      // The view-mode preference is an external store and notifies its
      // subscribers synchronously. Commit the captured anchor first so the
      // replacement surface cannot mount once without its restoration target.
      flushSync(() => {
        setLayoutScrollTarget(
          anchorId === null ? null : { messageId: anchorId, viewMode: mode },
        );
      });
      onModeChange?.(mode);
      setThreadViewMode(mode);
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          document
            .querySelector<HTMLElement>(
              restoreFocus
                ? '[data-testid="thread-view-mode-toggle"]'
                : '[data-testid="message-thread-body"]',
            )
            ?.focus({ preventScroll: true });
        });
      });
    },
    [onModeChange],
  );

  const resolveScrollTarget = React.useCallback(() => {
    const resolution = getResolvedThreadTargets({
      externalTargetId: externalScrollTargetId,
      layoutTargetId: layoutScrollTargetId,
    });
    if (resolution.resolveLayout) setLayoutScrollTarget(null);
    if (resolution.resolveExternal) onExternalTargetResolved();
  }, [externalScrollTargetId, layoutScrollTargetId, onExternalTargetResolved]);

  return {
    changeThreadViewMode,
    layoutScrollTargetId,
    resolveScrollTarget,
  };
}
