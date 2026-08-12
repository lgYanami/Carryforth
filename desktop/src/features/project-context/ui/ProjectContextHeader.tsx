import { FileText, Network, RefreshCw, ShieldCheck } from "lucide-react";

import type { ProjectContextQueryResult } from "@/shared/api/tauriProjectContext";
import { TopChromeInsetHeader } from "@/shared/layout/TopChromeInsetHeader";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";

/** Fixed Project Context page chrome above the full-canvas workspace. */
export function ProjectContextHeader({
  onRefresh,
  refreshing,
  result,
  syncBadge,
  syncState,
}: {
  onRefresh?: () => void;
  refreshing?: boolean;
  result?: ProjectContextQueryResult;
  syncBadge?: string;
  syncState?: "live" | "refreshing" | "stale";
}) {
  return (
    <TopChromeInsetHeader flush>
      <header
        className="flex h-12 items-center gap-2 px-3 sm:gap-3 sm:px-5"
        data-tauri-drag-region
      >
        <Network className="h-4 w-4 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold">Project Context</div>
          <div className="hidden text-2xs text-muted-foreground sm:block">
            Verified, read-only relationships across project coordinates
          </div>
        </div>
        {result ? (
          <Badge variant="success">
            <ShieldCheck className="mr-1 h-3 w-3" />
            Verified
          </Badge>
        ) : null}
        {result && !result.context.capabilityEnabled ? (
          <Badge variant="warning">Capability off · read-only</Badge>
        ) : null}
        {result ? (
          <Badge className="hidden sm:inline-flex" variant="outline">
            Revision {result.context.contextRevision}
          </Badge>
        ) : null}
        {syncState ? (
          <Badge
            data-testid="project-context-sync-status"
            variant={
              syncState === "stale"
                ? "warning"
                : syncState === "live"
                  ? "success"
                  : "secondary"
            }
          >
            {syncState === "refreshing" ? (
              <RefreshCw className="mr-1 h-3 w-3 animate-spin" />
            ) : null}
            {syncBadge ??
              (syncState === "stale"
                ? "Stale"
                : syncState === "live"
                  ? "Live"
                  : "Syncing")}
          </Badge>
        ) : null}
        {onRefresh ? (
          <Button
            aria-label="Refresh Project Context"
            data-testid="project-context-refresh"
            disabled={refreshing}
            onClick={onRefresh}
            size="icon"
            type="button"
            variant="ghost"
          >
            <RefreshCw
              className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`}
            />
          </Button>
        ) : null}
      </header>
    </TopChromeInsetHeader>
  );
}

/** Keeps the last verified snapshot explanation above the canvas. */
export function ProjectContextSyncBanner({
  message,
  state,
}: {
  message?: string;
  state?: "live" | "refreshing" | "stale";
}) {
  if (!message || (state !== "stale" && state !== "refreshing")) return null;

  return state === "stale" ? (
    <div
      className="flex items-start gap-2 border-b border-warning/30 bg-warning/10 px-4 py-2 text-xs text-muted-foreground"
      data-testid="project-context-stale-message"
    >
      <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0" />
      <span>{message}</span>
    </div>
  ) : (
    <div
      className="border-b border-border/70 bg-muted/20 px-4 py-2 text-xs text-muted-foreground"
      data-testid="project-context-refreshing"
    >
      {message}
    </div>
  );
}
