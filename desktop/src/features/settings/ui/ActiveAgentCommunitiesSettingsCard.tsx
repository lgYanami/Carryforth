import * as React from "react";

import { useManagedAgentsQuery } from "@/features/agents/hooks";
import {
  useManagedAgentRuntimeAction,
  useManagedAgentRuntimesQuery,
} from "@/features/agents/managedAgentRuntimeHooks";
import {
  agentCommunityAvailability,
  agentCommunityStatusDetail,
  managedAgentConnectionRelayUrl,
  managedAgentRuntimeKey,
  runtimeSupervisionImpact,
  runtimeSupervisionLabel,
  runtimeSupervisorOperatorCommand,
} from "@/features/agents/managedAgentRuntimeStatus";
import type { ManagedAgentRuntimeStatus } from "@/shared/api/types";
import { prepareRuntimeSupervisorIdentity } from "@/shared/api/tauriManagedAgents";
import { Button } from "@/shared/ui/button";
import { Badge } from "@/shared/ui/badge";
import { truncatePubkey } from "@/shared/lib/pubkey";
import { SettingsSectionHeader } from "./SettingsSectionHeader";

export function ActiveAgentCommunitiesSettingsCard() {
  const agentsQuery = useManagedAgentsQuery();
  const runtimesQuery = useManagedAgentRuntimesQuery();
  const action = useManagedAgentRuntimeAction();
  const [pendingRuntimeKey, setPendingRuntimeKey] = React.useState<
    string | null
  >(null);
  const [supervisionError, setSupervisionError] = React.useState<string | null>(
    null,
  );
  const [copiedRuntimeKey, setCopiedRuntimeKey] = React.useState<string | null>(
    null,
  );

  const agentNames = React.useMemo(
    () =>
      new Map(
        (agentsQuery.data ?? []).map((agent) => [
          agent.pubkey.toLowerCase(),
          agent.name,
        ]),
      ),
    [agentsQuery.data],
  );
  const runtimes = runtimesQuery.data ?? [];

  async function runAction(runtime: ManagedAgentRuntimeStatus) {
    setPendingRuntimeKey(managedAgentRuntimeKey(runtime));
    try {
      const connectionRelayUrl = managedAgentConnectionRelayUrl(runtime);
      await action.mutateAsync({
        action:
          runtime.lifecycle === "starting" ||
          runtime.lifecycle === "listening" ||
          runtime.lifecycle === "waking" ||
          runtime.lifecycle === "ready"
            ? "stop"
            : runtime.lifecycle === "stopped"
              ? "start"
              : "restart",
        pubkey: runtime.pubkey,
        relayUrl: connectionRelayUrl,
      });
    } finally {
      setPendingRuntimeKey(null);
    }
  }

  async function prepareSupervisor(runtime: ManagedAgentRuntimeStatus) {
    const runtimeKey = managedAgentRuntimeKey(runtime);
    setPendingRuntimeKey(runtimeKey);
    setSupervisionError(null);
    try {
      const connectionRelayUrl = managedAgentConnectionRelayUrl(runtime);
      await prepareRuntimeSupervisorIdentity({
        relayUrl: connectionRelayUrl,
        agentPubkey: runtime.pubkey,
      });
      if (runtime.lifecycle !== "stopped") {
        await action.mutateAsync({
          action: "restart",
          pubkey: runtime.pubkey,
          relayUrl: connectionRelayUrl,
        });
      } else {
        await runtimesQuery.refetch();
      }
    } catch (error) {
      setSupervisionError(
        error instanceof Error
          ? error.message
          : "Could not prepare Supervisor.",
      );
    } finally {
      setPendingRuntimeKey(null);
    }
  }

  async function copyBindingCommand(runtime: ManagedAgentRuntimeStatus) {
    const supervision = runtime.supervision;
    if (!supervision?.assignmentId || !supervision.localSupervisorPubkey)
      return;
    setSupervisionError(null);
    try {
      const command = runtimeSupervisorOperatorCommand(runtime);
      if (!command)
        throw new Error("Runtime supervision coordinates are incomplete.");
      await navigator.clipboard.writeText(command);
      const runtimeKey = managedAgentRuntimeKey(runtime);
      setCopiedRuntimeKey(runtimeKey);
      window.setTimeout(() => setCopiedRuntimeKey(null), 1500);
    } catch (error) {
      setSupervisionError(
        error instanceof Error
          ? error.message
          : "Could not copy the Supervisor command.",
      );
    }
  }

  return (
    <section className="min-w-0" data-testid="active-agent-communities">
      <SettingsSectionHeader
        title="Active in communities"
        description="See and control each community where this device runs your agents."
      />
      <div className="overflow-hidden rounded-xl border border-border/60">
        {runtimesQuery.isPending ? (
          <p className="px-4 py-3 text-sm text-muted-foreground">Loading…</p>
        ) : runtimes.length === 0 ? (
          <p className="px-4 py-3 text-sm text-muted-foreground">
            No agent community runtimes found.
          </p>
        ) : (
          runtimes.map((runtime) => {
            const status = agentCommunityAvailability(runtime);
            const detail = agentCommunityStatusDetail(runtime);
            const runtimeKey = managedAgentRuntimeKey(runtime);
            const pending = pendingRuntimeKey === runtimeKey;
            const canPrepareSupervisor =
              runtime.supervision?.state === "disabled" ||
              runtime.supervision?.state === "degraded_missing_key";
            const canCopyBinding = Boolean(
              runtime.supervision?.assignmentId &&
                runtime.supervision?.localSupervisorPubkey &&
                (runtime.supervision?.state === "awaiting_binding" ||
                  runtime.supervision?.state === "degraded_mismatch"),
            );
            return (
              <div
                className="flex items-center gap-3 border-b border-border/60 px-4 py-3 last:border-b-0"
                data-pubkey={runtime.pubkey}
                data-relay-url={runtime.relayUrl}
                data-testid="agent-community-runtime"
                key={runtimeKey}
              >
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="text-sm font-medium">
                      {agentNames.get(runtime.pubkey.toLowerCase()) ??
                        truncatePubkey(runtime.pubkey)}
                    </p>
                    <Badge
                      variant={status === "Here" ? "default" : "secondary"}
                    >
                      {status}
                    </Badge>
                  </div>
                  <p className="truncate text-xs text-muted-foreground">
                    {runtime.relayUrl}
                  </p>
                  {detail ? (
                    <p className="text-xs text-muted-foreground">{detail}</p>
                  ) : null}
                  <div className="mt-2 flex flex-wrap items-center gap-2">
                    <Badge
                      variant={
                        runtime.supervision?.state === "active"
                          ? "default"
                          : "secondary"
                      }
                    >
                      {runtimeSupervisionLabel(runtime)}
                    </Badge>
                    {runtime.supervision?.localSupervisorPubkey ? (
                      <span className="text-xs text-muted-foreground">
                        {truncatePubkey(
                          runtime.supervision.localSupervisorPubkey,
                        )}
                      </span>
                    ) : null}
                  </div>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {runtimeSupervisionImpact(runtime)}
                  </p>
                </div>
                <div className="flex shrink-0 flex-col gap-2">
                  {canPrepareSupervisor ? (
                    <Button
                      disabled={pending}
                      onClick={() => void prepareSupervisor(runtime)}
                      size="sm"
                      type="button"
                      variant="outline"
                    >
                      Prepare Supervisor
                    </Button>
                  ) : null}
                  {canCopyBinding ? (
                    <Button
                      disabled={pending}
                      onClick={() => void copyBindingCommand(runtime)}
                      size="sm"
                      type="button"
                      variant="outline"
                    >
                      {copiedRuntimeKey === runtimeKey
                        ? "Copied"
                        : runtime.supervision?.state === "degraded_mismatch"
                          ? "Copy repair command"
                          : "Copy bind command"}
                    </Button>
                  ) : null}
                  {runtime.localSetup ? (
                    <Button
                      disabled={pending}
                      onClick={() => void runAction(runtime)}
                      size="sm"
                      type="button"
                      variant="outline"
                    >
                      {pending
                        ? "Working…"
                        : runtime.lifecycle === "stopped"
                          ? "Start"
                          : runtime.lifecycle === "failed"
                            ? "Restart"
                            : "Stop"}
                    </Button>
                  ) : null}
                </div>
              </div>
            );
          })
        )}
      </div>
      {action.error instanceof Error ? (
        <p className="mt-2 text-sm text-destructive">{action.error.message}</p>
      ) : null}
      {supervisionError ? (
        <p className="mt-2 text-sm text-destructive">{supervisionError}</p>
      ) : null}
    </section>
  );
}
