import * as React from "react";

const DEFAULT_WIDTH_PX = 384;
const MIN_WIDTH_PX = 320;
const MAX_WIDTH_PX = 640;
const KEYBOARD_STEP_PX = 32;

function clampWidth(width: number): number {
  return Math.max(MIN_WIDTH_PX, Math.min(MAX_WIDTH_PX, width));
}

function storedWidth(storageKey: string): number {
  const parsed = Number.parseInt(
    window.sessionStorage.getItem(storageKey) ?? "",
    10,
  );
  return Number.isFinite(parsed) ? clampWidth(parsed) : DEFAULT_WIDTH_PX;
}

export function useResizableMeetingBoardWidth(communityId?: string) {
  const storageKey = `buzz.desktop.meeting-board-width.${communityId ?? "no-community"}`;
  const stopResizeRef = React.useRef<(() => void) | null>(null);
  const [widthPx, setWidthPx] = React.useState(() => {
    try {
      return storedWidth(storageKey);
    } catch {
      return DEFAULT_WIDTH_PX;
    }
  });

  React.useEffect(() => {
    try {
      window.sessionStorage.setItem(storageKey, String(widthPx));
    } catch {
      // The in-memory preference remains usable when storage is unavailable.
    }
  }, [storageKey, widthPx]);

  React.useEffect(
    () => () => {
      stopResizeRef.current?.();
    },
    [],
  );

  const onResizeStart = React.useCallback(
    (event: React.PointerEvent<HTMLButtonElement>) => {
      event.preventDefault();
      stopResizeRef.current?.();
      const startX = event.clientX;
      const startWidth = widthPx;
      const previousCursor = document.body.style.cursor;
      const previousUserSelect = document.body.style.userSelect;
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";

      const onMove = (moveEvent: PointerEvent) => {
        setWidthPx(clampWidth(startWidth + startX - moveEvent.clientX));
      };
      const onEnd = () => {
        document.body.style.cursor = previousCursor;
        document.body.style.userSelect = previousUserSelect;
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onEnd);
        window.removeEventListener("pointercancel", onEnd);
        stopResizeRef.current = null;
      };
      stopResizeRef.current = onEnd;
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onEnd);
      window.addEventListener("pointercancel", onEnd);
    },
    [widthPx],
  );

  const onResizeKeyDown = React.useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>) => {
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        setWidthPx((current) => clampWidth(current + KEYBOARD_STEP_PX));
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        setWidthPx((current) => clampWidth(current - KEYBOARD_STEP_PX));
      } else if (event.key === "Home") {
        event.preventDefault();
        setWidthPx(DEFAULT_WIDTH_PX);
      }
    },
    [],
  );

  return {
    onResizeKeyDown,
    onResizeStart,
    reset: React.useCallback(() => setWidthPx(DEFAULT_WIDTH_PX), []),
    widthPx,
  };
}
