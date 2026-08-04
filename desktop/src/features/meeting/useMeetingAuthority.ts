import * as React from "react";
import type { QueryObserverResult } from "@tanstack/react-query";

import { useRelayConnection } from "@/shared/api/useRelayConnection";

export type MeetingAuthorityStatus = "current" | "stale" | "resyncing";

type RecoveryState = "current" | "disconnected" | "resyncing" | "error";

/**
 * Keep Meeting controls behind a successful authoritative read after every
 * connection interruption. Cached content remains useful while stale, but it
 * never grants a Floor, Board, host, or action-finalization window.
 */
export function useMeetingAuthority<T>(input: {
  hasVerifiedSnapshot: boolean;
  readError: boolean;
  refetch: () => Promise<QueryObserverResult<T, Error>>;
  scopeKey: string;
}) {
  const { hasVerifiedSnapshot, readError, refetch, scopeKey } = input;
  const connectionState = useRelayConnection({ degradedAfterMs: 0 });
  const connectionRef = React.useRef(connectionState);
  const scopeRef = React.useRef(scopeKey);
  const [recoveryState, setRecoveryState] =
    React.useState<RecoveryState>("current");

  connectionRef.current = connectionState;

  React.useEffect(() => {
    scopeRef.current = scopeKey;
    setRecoveryState("current");
  }, [scopeKey]);

  React.useEffect(() => {
    if (!hasVerifiedSnapshot) return;
    if (connectionState !== "connected") {
      setRecoveryState("disconnected");
    }
  }, [connectionState, hasVerifiedSnapshot]);

  React.useEffect(() => {
    if (hasVerifiedSnapshot && readError) {
      setRecoveryState((current) =>
        current === "current" ? "error" : current,
      );
    }
  }, [hasVerifiedSnapshot, readError]);

  const resync = React.useCallback(async () => {
    if (connectionRef.current !== "connected") return false;
    const requestedScope = scopeRef.current;
    setRecoveryState("resyncing");
    const result = await refetch();
    if (scopeRef.current !== requestedScope) return false;
    if (connectionRef.current !== "connected") {
      setRecoveryState("disconnected");
      return false;
    }
    if (result.error) {
      setRecoveryState("error");
      return false;
    }
    setRecoveryState("current");
    return true;
  }, [refetch]);

  React.useEffect(() => {
    if (connectionState === "connected" && recoveryState === "disconnected") {
      void resync();
    }
  }, [connectionState, recoveryState, resync]);

  const status: MeetingAuthorityStatus =
    connectionState !== "connected" || recoveryState === "disconnected"
      ? "stale"
      : recoveryState === "resyncing"
        ? "resyncing"
        : recoveryState === "error" || readError
          ? "stale"
          : "current";

  return {
    authorityAvailable: hasVerifiedSnapshot && status === "current",
    canRetry: connectionState === "connected" && status === "stale",
    retry: resync,
    status,
  };
}
