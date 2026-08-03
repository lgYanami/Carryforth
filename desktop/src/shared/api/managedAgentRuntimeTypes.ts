export type ManagedAgentRuntimeLifecycle =
  | "starting"
  | "listening"
  | "waking"
  | "ready"
  | "failed"
  | "stopped";

export type ManagedAgentRuntimeSupervisionState =
  | "not_applicable"
  | "disabled"
  | "awaiting_binding"
  | "starting"
  | "active"
  | "recovering"
  | "degraded_missing_key"
  | "degraded_mismatch"
  | "expired"
  | "unavailable"
  | "unknown";

export type RuntimeSupervisorIdentitySource =
  | "environment"
  | "keyring"
  | "restricted_file";

export type ManagedAgentRuntimeSupervisionStatus = {
  state: ManagedAgentRuntimeSupervisionState;
  connectionRelayUrl: string | null;
  assignmentId: string | null;
  bindingId: string | null;
  supervisorPubkey: string | null;
  localSupervisorPubkey: string | null;
  identityAvailability:
    | "missing"
    | "ready"
    | "locked"
    | "lost"
    | "invalid"
    | null;
  identitySource: RuntimeSupervisorIdentitySource | null;
  identityDetailCode: string | null;
  runtimeId: string | null;
  runtimeEpoch: number | null;
  leaseExpiresAt: string | null;
  detailCode: string | null;
  observedAt: string;
  stale: boolean;
};

export type RuntimeSupervisorIdentityStatus = {
  relayUrl: string;
  availability: "missing" | "ready" | "locked" | "lost" | "invalid";
  publicKey?: string;
  source?: RuntimeSupervisorIdentitySource;
  detailCode?: string;
};

export type ManagedAgentRuntimeStatus = {
  pubkey: string;
  /** Exact submitted descriptor, present only on startup reconcile results. */
  requestedRelayUrl?: string;
  /** Canonical, backend-owned pair identity component. Do not normalize in TS. */
  relayUrl: string;
  localSetup: boolean;
  lifecycle: ManagedAgentRuntimeLifecycle;
  pid: number | null;
  error: string | null;
  logPath: string | null;
  supervision?: ManagedAgentRuntimeSupervisionStatus;
};
