import * as React from "react";

import { useMeetingActionFinalizationMutation } from "@/features/meeting/hooks";
import {
  clearMeetingPendingCommand,
  readMeetingPendingCommand,
  writeMeetingPendingCommand,
} from "@/features/meeting/meetingPendingCommandStore";
import type {
  MeetingActionFinalizationAction,
  MeetingActionFinalizationInput,
  MeetingActionFinalizationResult,
  MeetingSnapshot,
} from "@/shared/api/tauriMeetings";

export type MeetingActionFinalizationController = {
  disabled: boolean;
  error: Error | null;
  isPending: boolean;
  unresolved: MeetingActionFinalizationInput | null;
  resetError: () => void;
  retryExact: () => Promise<void>;
  submit: (
    action: MeetingActionFinalizationAction,
  ) => Promise<MeetingActionFinalizationResult | undefined>;
};

export function useMeetingActionFinalizationController(input: {
  scopeKey: string;
  snapshot: MeetingSnapshot;
}): MeetingActionFinalizationController {
  const { scopeKey, snapshot } = input;
  const mutation = useMeetingActionFinalizationMutation(snapshot.meetingId);
  const { error, isPending, mutateAsync, reset } = mutation;
  const [unresolved, setUnresolvedState] =
    React.useState<MeetingActionFinalizationInput | null>(() =>
      readMeetingPendingCommand<MeetingActionFinalizationInput>(
        scopeKey,
        "action",
        snapshot.meetingId,
      ),
    );
  const setUnresolved = React.useCallback(
    (value: MeetingActionFinalizationInput | null) => {
      setUnresolvedState(value);
      if (value) {
        writeMeetingPendingCommand(scopeKey, "action", value);
      } else {
        clearMeetingPendingCommand(scopeKey, "action");
      }
    },
    [scopeKey],
  );

  const handleResult = React.useCallback(
    (
      submitted: MeetingActionFinalizationInput,
      result: MeetingActionFinalizationResult,
    ): MeetingActionFinalizationResult => {
      setUnresolved(result.status === "indeterminate" ? submitted : null);
      return result;
    },
    [setUnresolved],
  );

  const submit = React.useCallback(
    async (action: MeetingActionFinalizationAction) => {
      if (!snapshot.host || unresolved) return undefined;
      reset();
      const submitted: MeetingActionFinalizationInput = {
        submissionId: crypto.randomUUID(),
        meetingId: snapshot.meetingId,
        expectedControlToken: snapshot.host.controlToken,
        action,
      };
      try {
        const result = await mutateAsync(submitted);
        return handleResult(submitted, result);
      } catch {
        // Native has released a definitively rejected submission. A later
        // attempt must be signed with a new submission identity.
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
