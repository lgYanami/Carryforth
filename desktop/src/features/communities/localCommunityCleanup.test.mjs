import assert from "node:assert/strict";
import test from "node:test";

import { removeRelayScopedStorageKeys } from "./localCommunityCleanup.ts";

function memoryStorage(initial) {
  const values = new Map(Object.entries(initial));
  return {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
    key: (index) => Array.from(values.keys())[index] ?? null,
    get length() {
      return values.size;
    },
  };
}

test("remote relay cleanup removes raw and encoded cache scopes only", () => {
  const remote = "wss://relay.example";
  const encoded = encodeURIComponent(remote);
  const storage = memoryStorage({
    [`buzz-thread-activity.v1:${remote}:pubkey`]: "activity",
    [`buzz-channel-sections.v1:pubkey:${encoded}`]: "sections",
    "buzz-channels.v1:ws://localhost:3000": "local",
    "buzz-machine-onboarding-complete.v2:pubkey": "true",
  });

  removeRelayScopedStorageKeys(`${remote}/`, storage);

  assert.equal(
    storage.getItem(`buzz-thread-activity.v1:${remote}:pubkey`),
    null,
  );
  assert.equal(
    storage.getItem(`buzz-channel-sections.v1:pubkey:${encoded}`),
    null,
  );
  assert.equal(
    storage.getItem("buzz-channels.v1:ws://localhost:3000"),
    "local",
  );
  assert.equal(
    storage.getItem("buzz-machine-onboarding-complete.v2:pubkey"),
    "true",
  );
});

test("duplicate local relay cleanup never removes retained local snapshots", () => {
  const storage = memoryStorage({
    "buzz-channels.v1:ws://localhost:3000": "canonical",
    "buzz-channels.v1:ws://127.0.0.1:3000": "alias",
  });

  removeRelayScopedStorageKeys("ws://127.0.0.1:3000", storage);

  assert.equal(
    storage.getItem("buzz-channels.v1:ws://localhost:3000"),
    "canonical",
  );
  assert.equal(
    storage.getItem("buzz-channels.v1:ws://127.0.0.1:3000"),
    "alias",
  );
});
