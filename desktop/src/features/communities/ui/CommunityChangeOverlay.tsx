import * as React from "react";

import { CANONICAL_LOCAL_RELAY_URL } from "@/shared/runtime/desktopNetworkMode";
import { Button } from "@/shared/ui/button";

type CommunityChangeOverlayProps = {
  onClose: () => void;
};

export function CommunityChangeOverlay({
  onClose,
}: CommunityChangeOverlayProps) {
  const overlayRef = React.useRef<HTMLDivElement>(null);

  // Focus trap: focus the overlay on mount
  React.useEffect(() => {
    overlayRef.current?.focus();
  }, []);

  // Escape key closes the overlay
  React.useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  return (
    <div
      aria-modal="true"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"
      data-testid="community-change-overlay"
      ref={overlayRef}
      role="dialog"
      tabIndex={-1}
    >
      {/* Background click closes */}
      <div aria-hidden="true" className="absolute inset-0" onClick={onClose} />
      <div className="relative z-10 w-full max-w-md rounded-2xl border border-border bg-background p-8 shadow-2xl">
        <h2 className="text-xl font-semibold tracking-tight">
          Local community
        </h2>
        <p className="mt-2 text-sm text-muted-foreground">
          Carryforth uses the local Relay at {CANONICAL_LOCAL_RELAY_URL}. Remote
          communities cannot be added or selected.
        </p>
        <Button className="mt-6 w-full" onClick={onClose} type="button">
          Close
        </Button>
      </div>
    </div>
  );
}
