import * as React from "react";

import {
  projectContextDraftFromQuery,
  type ProjectContextQueryDraft,
} from "@/features/project-context/queryModel";
import type { ProjectContextSemanticOverlay } from "@/features/project-context/semanticOverlay";
import type {
  SemanticAttempt,
  SemanticSession,
} from "@/features/project-context/semanticSession";
import type { ProjectContextRouteSelection } from "@/features/project-context/routeState";
import type { ProjectContextWorkspaceAnnouncementEvent } from "@/features/project-context/workspacePanelModel";
import {
  projectContextQueryKey,
  type ProjectContextQuery,
} from "@/shared/api/tauriProjectContext";

/** Workspace-owned structural draft synchronized only when the applied route query changes. */
export function useProjectContextStructuralDraft(
  appliedQuery: ProjectContextQuery,
) {
  const appliedKey = projectContextQueryKey(appliedQuery);
  const lastAppliedKey = React.useRef(appliedKey);
  const [draft, setDraft] = React.useState<ProjectContextQueryDraft>(() =>
    projectContextDraftFromQuery(appliedQuery),
  );

  React.useEffect(() => {
    if (lastAppliedKey.current === appliedKey) return;
    lastAppliedKey.current = appliedKey;
    setDraft(projectContextDraftFromQuery(appliedQuery));
  }, [appliedKey, appliedQuery]);

  return [draft, setDraft] as const;
}

/** Single deduplicated dynamic announcement source for the workspace shell. */
export function useProjectContextWorkspaceAnnouncement({
  active,
  attempt,
  selection,
  workspaceStateEvent,
}: {
  active: SemanticSession<ProjectContextSemanticOverlay> | null;
  attempt: SemanticAttempt;
  selection: ProjectContextRouteSelection | null;
  workspaceStateEvent?: { key: string; message: string };
}) {
  const [announcement, setAnnouncement] = React.useState<{
    key: string;
    message: string;
  }>();
  const selectionKind = selection?.kind;
  const selectionKey = selection?.key;
  const previousSelectionRef = React.useRef<{
    kind: ProjectContextRouteSelection["kind"];
    key: string;
  } | null>(null);
  const clearGenerationRef = React.useRef(0);
  const workspaceStateKey = workspaceStateEvent?.key;
  const workspaceStateMessage = workspaceStateEvent?.message;
  const workspaceStateGenerationRef = React.useRef(0);
  const previousWorkspaceStateKeyRef = React.useRef<string | undefined>(
    undefined,
  );

  React.useEffect(() => {
    if (attempt.status === "running") {
      setAnnouncement({
        key: `semantic:${attempt.token}:running`,
        message: "Finding semantic paths.",
      });
    } else if (attempt.status === "pairing") {
      setAnnouncement({
        key: `semantic:${attempt.token}:pairing`,
        message: "Pairing verified semantic paths with All Context.",
      });
    } else if (attempt.status === "failed") {
      setAnnouncement({
        key: `semantic:${attempt.token}:failed`,
        message: `Semantic query failed. ${attempt.error.message}`,
      });
    }
  }, [attempt]);

  React.useEffect(() => {
    if (!active) return;
    const paths = active.verifiedDisplayResult.coverage.pathsReturned;
    const roots = active.verifiedDisplayResult.coverage.rootsReturned;
    setAnnouncement({
      key: `semantic:${active.requestId}:active`,
      message: `${paths} semantic ${paths === 1 ? "path" : "paths"} and ${roots} ${roots === 1 ? "root" : "roots"} ready.`,
    });
  }, [active]);

  React.useEffect(() => {
    const previous = previousSelectionRef.current;
    previousSelectionRef.current =
      selectionKind && selectionKey
        ? { kind: selectionKind, key: selectionKey }
        : null;
    if (!selectionKind || !selectionKey) {
      if (previous) {
        setAnnouncement({
          key: `selection:${previous.kind}:${previous.key}:cleared`,
          message: "Graph selection cleared.",
        });
      }
      return;
    }
    setAnnouncement({
      key: `selection:${selectionKind}:${selectionKey}:selected`,
      message:
        selectionKind === "coordinate"
          ? "Coordinate details selected."
          : "Context Edge details selected.",
    });
  }, [selectionKey, selectionKind]);

  React.useEffect(() => {
    if (!workspaceStateKey || !workspaceStateMessage) {
      previousWorkspaceStateKeyRef.current = undefined;
      return;
    }
    if (previousWorkspaceStateKeyRef.current === workspaceStateKey) return;
    previousWorkspaceStateKeyRef.current = workspaceStateKey;
    workspaceStateGenerationRef.current += 1;
    setAnnouncement({
      key: `${workspaceStateKey}:${workspaceStateGenerationRef.current}`,
      message: workspaceStateMessage,
    });
  }, [workspaceStateKey, workspaceStateMessage]);

  const announceCleared = React.useCallback(() => {
    clearGenerationRef.current += 1;
    setAnnouncement({
      key: `semantic:cleared:${clearGenerationRef.current}`,
      message: "Semantic result cleared.",
    });
  }, []);

  const announceEvent = React.useCallback(
    (event: ProjectContextWorkspaceAnnouncementEvent) => {
      const key = event.key.trim();
      const message = event.message.trim();
      if (!key || !message) return;
      setAnnouncement({ key, message });
    },
    [],
  );

  return { announcement, announceCleared, announceEvent };
}
