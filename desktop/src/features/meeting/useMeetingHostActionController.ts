import * as React from "react";

import { useMeetingHostActionMutation } from "@/features/meeting/hooks";
import {
  clearMeetingPendingCommand,
  readMeetingPendingCommand,
  writeMeetingPendingCommand,
} from "@/features/meeting/meetingPendingCommandStore";
import type {
  MeetingHostAction,
  MeetingHostActionInput,
  MeetingHostActionResult,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";

export type MeetingHostActionController = {
  disabled: boolean;
  error: Error | null;
  isPending: boolean;
  unresolved: MeetingHostActionInput | null;
  resetError: () => void;
  retryExact: () => Promise<void>;
  submit: (
    action: MeetingHostAction,
  ) => Promise<MeetingHostActionResult | undefined>;
};

export function useMeetingHostActionController(input: {
  onBoardAccepted: () => void;
  scopeKey: string;
  snapshot: MeetingSnapshot;
}): MeetingHostActionController {
  const { onBoardAccepted, scopeKey, snapshot } = input;
  const mutation = useMeetingHostActionMutation(snapshot.meetingId);
  const { error, isPending, mutateAsync, reset } = mutation;
  const [unresolved, setUnresolvedState] =
    React.useState<MeetingHostActionInput | null>(() =>
      readMeetingPendingCommand<MeetingHostActionInput>(
        scopeKey,
        "host",
        snapshot.meetingId,
      ),
    );
  const setUnresolved = React.useCallback(
    (value: MeetingHostActionInput | null) => {
      setUnresolvedState(value);
      if (value) {
        writeMeetingPendingCommand(scopeKey, "host", value);
      } else {
        clearMeetingPendingCommand(scopeKey, "host");
      }
    },
    [scopeKey],
  );

  const handleResult = React.useCallback(
    (
      submitted: MeetingHostActionInput,
      result: MeetingHostActionResult,
    ): MeetingHostActionResult => {
      if (result.status === "indeterminate") {
        setUnresolved(submitted);
      } else {
        setUnresolved(null);
        if (
          result.action === "board_update" ||
          result.action === "board_unchanged"
        ) {
          onBoardAccepted();
        }
      }
      return result;
    },
    [onBoardAccepted, setUnresolved],
  );

  const submit = React.useCallback(
    async (action: MeetingHostAction) => {
      if (!snapshot.host || unresolved) return undefined;
      reset();
      const submitted: MeetingHostActionInput = {
        submissionId: crypto.randomUUID(),
        meetingId: snapshot.meetingId,
        expectedControlToken: snapshot.host.controlToken,
        action,
      };
      try {
        const result = await mutateAsync(submitted);
        return handleResult(submitted, result);
      } catch {
        // A definitive rejection releases the native pending command. A later
        // action must therefore receive a new submission identity.
        return undefined;
      }
    },
    [
      handleResult,
      mutateAsync,
      reset,
      snapshot.host,
      snapshot.meetingId,
      unresolved,
    ],
  );

  const retryExact = React.useCallback(async () => {
    if (!unresolved) return;
    reset();
    try {
      const result = await mutateAsync(unresolved);
      handleResult(unresolved, result);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (
        !message.includes("belongs to a different Community") &&
        !message.includes("belongs to a different identity")
      ) {
        setUnresolved(null);
      }
    }
  }, [handleResult, mutateAsync, reset, setUnresolved, unresolved]);

  return {
    disabled: isPending || unresolved !== null,
    error:
      error instanceof Error ? error : error ? new Error(String(error)) : null,
    isPending,
    unresolved,
    resetError: reset,
    retryExact,
    submit,
  };
}
