import * as React from "react";

import { useMeetingActionFinalizationMutation } from "@/features/meeting/hooks";
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

export function useMeetingActionFinalizationController(
  snapshot: MeetingSnapshot,
): MeetingActionFinalizationController {
  const mutation = useMeetingActionFinalizationMutation(snapshot.meetingId);
  const { error, isPending, mutateAsync, reset } = mutation;
  const [unresolved, setUnresolved] =
    React.useState<MeetingActionFinalizationInput | null>(null);

  const handleResult = React.useCallback(
    (
      submitted: MeetingActionFinalizationInput,
      result: MeetingActionFinalizationResult,
    ): MeetingActionFinalizationResult => {
      setUnresolved(result.status === "indeterminate" ? submitted : null);
      return result;
    },
    [],
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
  }, [handleResult, mutateAsync, reset, unresolved]);

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
