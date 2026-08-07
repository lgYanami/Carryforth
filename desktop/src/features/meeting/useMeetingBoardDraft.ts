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
  scopeKey: string;
  snapshot: MeetingSnapshot | null;
}): MeetingBoardDraft {
  const { editable, scopeKey, snapshot } = input;
  const host = snapshot?.host;
  const nextToken = editable ? (host?.controlToken ?? null) : null;
  const nextWindow = editable ? (host?.boardControl.boardWindow ?? null) : null;
  const authoritativeBody = snapshot?.board.body ?? "";
  const [scopedState, setScopedState] = React.useState<{
    scopeKey: string;
    state: BoardDraftState;
  }>({ scopeKey, state: EMPTY_STATE });
  const state =
    scopedState.scopeKey === scopeKey ? scopedState.state : EMPTY_STATE;

  React.useEffect(() => {
    setScopedState((currentScope) => {
      const current =
        currentScope.scopeKey === scopeKey ? currentScope.state : EMPTY_STATE;
      if (nextToken && nextWindow !== null) {
        if (current.controlToken === nextToken) return currentScope;
        const stale = preserveDirtyDraft(current) ?? current.stale;
        return {
          scopeKey,
          state: {
            controlToken: nextToken,
            boardWindow: nextWindow,
            value: authoritativeBody,
            initialValue: authoritativeBody,
            stale,
          },
        };
      }
      if (!current.controlToken) {
        return currentScope.scopeKey === scopeKey
          ? currentScope
          : { scopeKey, state: EMPTY_STATE };
      }
      return {
        scopeKey,
        state: {
          ...current,
          controlToken: null,
          boardWindow: null,
          stale: preserveDirtyDraft(current) ?? current.stale,
        },
      };
    });
  }, [authoritativeBody, nextToken, nextWindow, scopeKey]);

  const updateState = React.useCallback(
    (update: (current: BoardDraftState) => BoardDraftState) => {
      setScopedState((currentScope) => ({
        scopeKey,
        state: update(
          currentScope.scopeKey === scopeKey ? currentScope.state : EMPTY_STATE,
        ),
      }));
    },
    [scopeKey],
  );

  return {
    controlToken: state.controlToken,
    value: state.value,
    initialValue: state.initialValue,
    setValue: React.useCallback(
      (value: string) => updateState((current) => ({ ...current, value })),
      [updateState],
    ),
    stale: state.stale,
    dismissStale: React.useCallback(
      () => updateState((current) => ({ ...current, stale: null })),
      [updateState],
    ),
    markAccepted: React.useCallback(
      () =>
        updateState((current) => ({
          ...current,
          controlToken: null,
          boardWindow: null,
          initialValue: current.value,
          stale: null,
        })),
      [updateState],
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
