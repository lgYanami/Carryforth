import assert from "node:assert/strict";
import test from "node:test";

import {
  CANONICAL_LOCAL_RELAY_URL,
  getDesktopNetworkMode,
  isDesktopRelayUrlAllowed,
} from "./desktopNetworkMode.ts";

test("Desktop network policy has one permanent local coordinate", () => {
  assert.equal(getDesktopNetworkMode(), "localOnly");
  assert.equal(isDesktopRelayUrlAllowed(CANONICAL_LOCAL_RELAY_URL), true);

  for (const rejected of [
    " ws://localhost:3000",
    "ws://localhost:3000/",
    "ws://127.0.0.1:3000",
    "ws://localhost:3001",
    "wss://localhost:3000",
    "wss://relay.example",
  ]) {
    assert.equal(isDesktopRelayUrlAllowed(rejected), false, rejected);
  }
});
