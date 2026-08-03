import assert from "node:assert/strict";
import test from "node:test";

import {
  agentCommunityAvailability,
  agentCommunityStatusDetail,
  canonicalRelayUrl,
  findManagedAgentRuntime,
  managedAgentConnectionRelayUrl,
  managedAgentRuntimeKey,
  runtimeSupervisionImpact,
  runtimeSupervisionLabel,
  runtimeSupervisorOperatorCommand,
} from "./managedAgentRuntimeStatus.ts";

const runtime = (overrides = {}) => ({
  pubkey: "aa",
  relayUrl: "wss://relay.example",
  localSetup: true,
  lifecycle: "ready",
  pid: 1,
  error: null,
  logPath: null,
  ...overrides,
});

test("projects every backend lifecycle to the four product labels", () => {
  assert.equal(agentCommunityAvailability(runtime()), "Here");
  for (const lifecycle of ["starting", "listening", "waking"]) {
    assert.equal(agentCommunityAvailability(runtime({ lifecycle })), "Waking");
  }
  for (const lifecycle of ["failed", "stopped"]) {
    assert.equal(
      agentCommunityAvailability(runtime({ lifecycle })),
      "Unavailable",
    );
  }
});

test("backend-authoritative local setup takes precedence", () => {
  assert.equal(
    agentCommunityAvailability(
      runtime({ localSetup: false, lifecycle: "ready" }),
    ),
    "Needs setup on this device",
  );
});

test("unavailable detail distinguishes stopped and failed", () => {
  assert.equal(
    agentCommunityStatusDetail(runtime({ lifecycle: "stopped" })),
    "Stopped by you",
  );
  assert.equal(
    agentCommunityStatusDetail(
      runtime({ lifecycle: "failed", error: "Relay timed out" }),
    ),
    "Relay timed out",
  );
});

test("pair key cannot collide at component boundaries", () => {
  assert.notEqual(
    managedAgentRuntimeKey(runtime({ pubkey: "ab", relayUrl: "c" })),
    managedAgentRuntimeKey(runtime({ pubkey: "a", relayUrl: "bc" })),
  );
});

test("selects one relay without collapsing same-pubkey pairs", () => {
  const runtimes = [
    runtime({ relayUrl: "wss://a.example", lifecycle: "ready" }),
    runtime({ relayUrl: "wss://b.example", lifecycle: "failed" }),
  ];
  assert.equal(
    findManagedAgentRuntime(runtimes, "AA", "wss://b.example")?.lifecycle,
    "failed",
  );
  assert.equal(
    findManagedAgentRuntime(runtimes, "aa", "wss://c.example"),
    undefined,
  );
});

test("canonicalRelayUrl mirrors the backend pair-key normalization", () => {
  // Loopback folding + default-port and trailing-slash stripping — the
  // standard dev setup that previously broke pair matching.
  assert.equal(canonicalRelayUrl("ws://localhost:3000"), "ws://127.0.0.1:3000");
  assert.equal(
    canonicalRelayUrl("WSS://Relay.Example:443/"),
    "wss://relay.example",
  );
  assert.equal(
    canonicalRelayUrl("ws://relay.example:80/"),
    "ws://relay.example",
  );
  assert.equal(
    canonicalRelayUrl("wss://relay.example/path/"),
    "wss://relay.example/path",
  );
  assert.equal(canonicalRelayUrl("ws://[::1]:3000"), "ws://127.0.0.1:3000");
  assert.equal(canonicalRelayUrl("https://relay.example"), null);
  assert.equal(canonicalRelayUrl("not a url"), null);
});

test("matches a stored community URL against canonical backend rows", () => {
  const runtimes = [
    runtime({ relayUrl: "ws://127.0.0.1:3000", lifecycle: "ready" }),
  ];
  assert.equal(
    findManagedAgentRuntime(runtimes, "aa", "ws://localhost:3000")?.lifecycle,
    "ready",
  );
  assert.equal(
    findManagedAgentRuntime(runtimes, "aa", "ws://localhost:3001"),
    undefined,
  );
});

test("runtime supervision remains separate from Agent availability", () => {
  const degraded = runtime({
    supervision: {
      state: "degraded_missing_key",
      connectionRelayUrl: "ws://localhost:3000",
      assignmentId: "12345678-0000-0000-0000-000000000000",
      bindingId: "87654321-0000-0000-0000-000000000000",
      supervisorPubkey: "bb",
      localSupervisorPubkey: null,
      identitySource: null,
      runtimeId: null,
      runtimeEpoch: null,
      leaseExpiresAt: null,
      detailCode: "missing_key",
      observedAt: new Date(0).toISOString(),
      stale: false,
    },
  });

  assert.equal(agentCommunityAvailability(degraded), "Here");
  assert.equal(runtimeSupervisionLabel(degraded), "Supervisor degraded");
  assert.match(
    runtimeSupervisionImpact(degraded),
    /Ordinary chat and Role operations remain available/,
  );
  assert.match(runtimeSupervisionImpact(degraded), /Assignment 12345678/);
});

test("active Runtime supervision reports its independent state", () => {
  const active = runtime({
    supervision: {
      state: "active",
      connectionRelayUrl: "wss://relay.example",
      assignmentId: null,
      bindingId: null,
      supervisorPubkey: null,
      localSupervisorPubkey: null,
      identitySource: null,
      runtimeId: null,
      runtimeEpoch: null,
      leaseExpiresAt: null,
      detailCode: null,
      observedAt: new Date(0).toISOString(),
      stale: false,
    },
  });

  assert.equal(runtimeSupervisionLabel(active), "Supervisor active");
  assert.equal(
    runtimeSupervisionImpact(active),
    "Runtime supervision does not change Role authority.",
  );
});

test("operator commands preserve the live localhost authority", () => {
  const awaiting = runtime({
    relayUrl: "ws://127.0.0.1:3000",
    supervision: {
      state: "awaiting_binding",
      connectionRelayUrl: "ws://localhost:3000",
      assignmentId: "12345678-0000-0000-0000-000000000000",
      bindingId: null,
      supervisorPubkey: null,
      localSupervisorPubkey: "bb".repeat(32),
      identityAvailability: "ready",
      identitySource: "keyring",
      identityDetailCode: null,
      runtimeId: null,
      runtimeEpoch: null,
      leaseExpiresAt: null,
      detailCode: null,
      observedAt: new Date(0).toISOString(),
      stale: false,
    },
  });
  assert.equal(managedAgentConnectionRelayUrl(awaiting), "ws://localhost:3000");
  assert.match(
    runtimeSupervisorOperatorCommand(awaiting),
    /--host localhost:3000/,
  );
  assert.doesNotMatch(
    runtimeSupervisorOperatorCommand(awaiting),
    /127\.0\.0\.1/,
  );

  const mismatch = runtime({
    ...awaiting,
    supervision: {
      ...awaiting.supervision,
      state: "degraded_mismatch",
      bindingId: "87654321-0000-0000-0000-000000000000",
    },
  });
  assert.match(runtimeSupervisorOperatorCommand(mismatch), / revoke /);
  assert.match(runtimeSupervisorOperatorCommand(mismatch), / && buzz-admin /);
});
