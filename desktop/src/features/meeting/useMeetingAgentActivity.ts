import * as React from "react";

import type { MeetingAgentActivityAgent } from "@/features/meeting/meetingAgentActivityModel";
import { normalizePubkey } from "@/shared/lib/pubkey";

type MeetingAgentActivityState = {
  boardWasOpen: boolean;
  pubkey: string | null;
  scopeKey: string;
};

type UseMeetingAgentActivityInput = {
  agents: readonly MeetingAgentActivityAgent[];
  boardPanelIsOverlay: boolean;
  boardSheetOpen: boolean;
  scopeKey: string;
  setBoardSheetOpen: (open: boolean) => void;
  setWideBoardOpen: (open: boolean) => void;
  wideBoardOpen: boolean;
};

/** Coordinate Meeting Agent Activity selection with the shared Board rail. */
export function useMeetingAgentActivity({
  agents,
  boardPanelIsOverlay,
  boardSheetOpen,
  scopeKey,
  setBoardSheetOpen,
  setWideBoardOpen,
  wideBoardOpen,
}: UseMeetingAgentActivityInput) {
  const [state, setState] = React.useState<MeetingAgentActivityState>({
    boardWasOpen: false,
    pubkey: null,
    scopeKey,
  });
  const selectedPubkey = state.scopeKey === scopeKey ? state.pubkey : null;
  const selectedAgent = React.useMemo(() => {
    if (!selectedPubkey) return null;
    const normalizedTarget = normalizePubkey(selectedPubkey);
    return (
      agents.find(
        (agent) => normalizePubkey(agent.pubkey) === normalizedTarget,
      ) ?? null
    );
  }, [agents, selectedPubkey]);

  const openAgentActivity = React.useCallback(
    (pubkey: string) => {
      const normalizedTarget = normalizePubkey(pubkey);
      if (
        !agents.some(
          (agent) => normalizePubkey(agent.pubkey) === normalizedTarget,
        )
      ) {
        return;
      }

      setState((current) => ({
        boardWasOpen:
          current.scopeKey === scopeKey && current.pubkey
            ? current.boardWasOpen
            : boardPanelIsOverlay
              ? boardSheetOpen
              : wideBoardOpen,
        pubkey,
        scopeKey,
      }));
      if (boardPanelIsOverlay) setBoardSheetOpen(false);
    },
    [
      agents,
      boardPanelIsOverlay,
      boardSheetOpen,
      scopeKey,
      setBoardSheetOpen,
      wideBoardOpen,
    ],
  );

  const closeAgentActivity = React.useCallback(() => {
    const restoreBoard =
      state.scopeKey === scopeKey &&
      state.pubkey !== null &&
      state.boardWasOpen;
    setState({ boardWasOpen: false, pubkey: null, scopeKey });
    if (boardPanelIsOverlay) {
      setBoardSheetOpen(restoreBoard);
    } else {
      setWideBoardOpen(restoreBoard);
    }
  }, [
    boardPanelIsOverlay,
    scopeKey,
    setBoardSheetOpen,
    setWideBoardOpen,
    state,
  ]);

  const showMeetingBoard = React.useCallback(() => {
    setState({ boardWasOpen: true, pubkey: null, scopeKey });
    if (boardPanelIsOverlay) {
      setBoardSheetOpen(true);
    } else {
      setWideBoardOpen(true);
    }
  }, [boardPanelIsOverlay, scopeKey, setBoardSheetOpen, setWideBoardOpen]);

  React.useEffect(() => {
    if (!selectedPubkey || selectedAgent) return;
    closeAgentActivity();
  }, [closeAgentActivity, selectedAgent, selectedPubkey]);

  return {
    closeAgentActivity,
    openAgentActivity,
    selectedAgent,
    selectedPubkey,
    showMeetingBoard,
  };
}
