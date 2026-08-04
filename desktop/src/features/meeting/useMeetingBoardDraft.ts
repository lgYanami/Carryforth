import * as React from "react";

import type { MeetingSnapshot } from "@/shared/api/tauriMeetings";

export type MeetingStaleBoardDraft = {
  body: string;
  boardWindow: number;
};

export type MeetingBoardDraft = {
  controlToken: string | null;
  value: string;
  initialValue: string;
  setValue: (value: string) => void;
  stale: MeetingStaleBoardDraft | null;
  dismissStale: () => void;
  markAccepted: () => void;
};

type BoardDraftState = {
  controlToken: string | null;
  boardWindow: number | null;
  value: string;
  initialValue: string;
  stale: MeetingStaleBoardDraft | null;
};

const EMPTY_STATE: BoardDraftState = {
  controlToken: null,
  boardWindow: null,
  value: "",
  initialValue: "",
  stale: null,
};

/**
 * Keep a Board draft bound to one authoritative Board window. Losing that
 * window preserves edited text for copying, but removes every submit token.
 */
export function useMeetingBoardDraft(input: {
  editable: boolean;
  snapshot: MeetingSnapshot | null;
}): MeetingBoardDraft {
  const { editable, snapshot } = input;
  const host = snapshot?.host;
  const nextToken = editable ? (host?.controlToken ?? null) : null;
  const nextWindow = editable ? (host?.boardControl.boardWindow ?? null) : null;
  const authoritativeBody = snapshot?.board.body ?? "";
  const [state, setState] = React.useState<BoardDraftState>(EMPTY_STATE);

  React.useEffect(() => {
    setState((current) => {
      if (nextToken && nextWindow !== null) {
        if (current.controlToken === nextToken) return current;
        const stale = preserveDirtyDraft(current) ?? current.stale;
        return {
          controlToken: nextToken,
          boardWindow: nextWindow,
          value: authoritativeBody,
          initialValue: authoritativeBody,
          stale,
        };
      }
      if (!current.controlToken) return current;
      return {
        ...current,
        controlToken: null,
        boardWindow: null,
        stale: preserveDirtyDraft(current) ?? current.stale,
      };
    });
  }, [authoritativeBody, nextToken, nextWindow]);

  return {
    controlToken: state.controlToken,
    value: state.value,
    initialValue: state.initialValue,
    setValue: React.useCallback(
      (value: string) => setState((current) => ({ ...current, value })),
      [],
    ),
    stale: state.stale,
    dismissStale: React.useCallback(
      () => setState((current) => ({ ...current, stale: null })),
      [],
    ),
    markAccepted: React.useCallback(
      () =>
        setState((current) => ({
          ...current,
          controlToken: null,
          boardWindow: null,
          initialValue: current.value,
          stale: null,
        })),
      [],
    ),
  };
}

function preserveDirtyDraft(
  state: BoardDraftState,
): MeetingStaleBoardDraft | null {
  if (
    state.boardWindow === null ||
    state.value === state.initialValue ||
    !state.value.trim()
  ) {
    return null;
  }
  return { body: state.value, boardWindow: state.boardWindow };
}
