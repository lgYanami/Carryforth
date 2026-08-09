import assert from "node:assert/strict";
import test from "node:test";

import {
  clearCommunityStorage,
  ensureLocalOnlyCommunityStorage,
  initFirstCommunity,
  migrateLegacyCommunityStorage,
  projectConnectableCommunities,
  resolveLocalOnlyCommunityState,
} from "./communityStorage.ts";

function createMemoryStorage(initial = {}) {
  const values = new Map(Object.entries(initial));
  return {
    values,
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
    clear: () => values.clear(),
    key: (index) => Array.from(values.keys())[index] ?? null,
    get length() {
      return values.size;
    },
  };
}

test("migrateLegacyCommunityStorage promotes current Buzz workspace state", () => {
  const storage = createMemoryStorage({
    "buzz-workspaces": '[{"id":"current"}]',
    "buzz-active-workspace-id": "current",
  });

  migrateLegacyCommunityStorage(storage);

  assert.equal(storage.getItem("buzz-communities"), '[{"id":"current"}]');
  assert.equal(storage.getItem("buzz-active-community-id"), "current");
});

test("migrateLegacyCommunityStorage does not overwrite new community state", () => {
  const storage = createMemoryStorage({
    "buzz-communities": '[{"id":"new"}]',
    "buzz-active-community-id": "new",
    "buzz-workspaces": '[{"id":"old"}]',
    "buzz-active-workspace-id": "old",
  });

  migrateLegacyCommunityStorage(storage);

  assert.equal(storage.getItem("buzz-communities"), '[{"id":"new"}]');
  assert.equal(storage.getItem("buzz-active-community-id"), "new");
});

test("local-only resolution discards a remote-only community list", () => {
  const remote = {
    id: "remote",
    name: "Buzz Cloud",
    relayUrl: "wss://relay.example.com",
    addedAt: "2026-01-01T00:00:00.000Z",
  };

  const result = resolveLocalOnlyCommunityState(
    [remote],
    remote.id,
    () => "local",
    () => "2026-08-09T00:00:00.000Z",
    "local-pubkey",
  );

  assert.deepEqual(result.storedCommunities, [
    {
      id: "local",
      name: "Local Dev",
      relayUrl: "ws://localhost:3000",
      pubkey: "local-pubkey",
      addedAt: "2026-08-09T00:00:00.000Z",
    },
  ]);
  assert.equal(result.activeId, "local");
  assert.deepEqual(result.removedCommunities, [remote]);
});

test("local-only resolution reuses one local record and removes every duplicate", () => {
  const remote = {
    id: "remote",
    name: "Remote",
    relayUrl: "wss://relay.example.com",
    addedAt: "2026-01-01T00:00:00.000Z",
  };
  const localAlias = {
    id: "stable-local",
    name: "Old Local",
    relayUrl: "ws://127.0.0.1:3000",
    token: "local-token",
    reposDir: "/tmp/repos",
    addedAt: "2026-01-02T00:00:00.000Z",
  };
  const duplicate = {
    id: "duplicate-local",
    name: "Duplicate",
    relayUrl: "ws://localhost:3000/",
    addedAt: "2026-01-03T00:00:00.000Z",
  };

  const result = resolveLocalOnlyCommunityState(
    [remote, localAlias, duplicate],
    remote.id,
    () => "unused",
    () => "unused",
    "current-pubkey",
  );

  assert.deepEqual(result.connectableCommunity, {
    ...localAlias,
    name: "Local Dev",
    relayUrl: "ws://localhost:3000",
    pubkey: "current-pubkey",
  });
  assert.deepEqual(
    result.removedCommunities.map((community) => community.id),
    ["remote", "duplicate-local"],
  );
  assert.deepEqual(projectConnectableCommunities(result.storedCommunities), [
    result.connectableCommunity,
  ]);
});

test("failed first-community write preserves existing community data", () => {
  const storage = createMemoryStorage({
    "buzz-communities": '[{"id":"existing"}]',
    "buzz-workspaces": '[{"id":"legacy"}]',
    "buzz-active-workspace-id": "legacy",
  });
  storage.setItem = (key, value) => {
    if (key === "buzz-communities") {
      throw new Error("QuotaExceededError");
    }
    storage.values.set(key, String(value));
  };
  globalThis.localStorage = storage;
  globalThis.window = { localStorage: storage };

  assert.equal(initFirstCommunity("ws://localhost:3000", "pubkey"), null);
  assert.equal(storage.getItem("buzz-communities"), '[{"id":"existing"}]');
  assert.equal(storage.getItem("buzz-active-community-id"), null);
  assert.equal(storage.getItem("buzz-workspaces"), '[{"id":"legacy"}]');
  assert.equal(storage.getItem("buzz-active-workspace-id"), "legacy");
});

test("local bootstrap removes remote and legacy state only after canonical writes", () => {
  const storage = createMemoryStorage({
    "buzz-communities": JSON.stringify([
      {
        id: "local",
        name: "Old Local",
        relayUrl: "ws://127.0.0.1:3000",
        addedAt: "2026-01-01T00:00:00.000Z",
      },
      {
        id: "remote",
        name: "Remote",
        relayUrl: "wss://relay.example.com",
        addedAt: "2026-01-02T00:00:00.000Z",
      },
    ]),
    "buzz-active-community-id": "remote",
    "buzz-workspaces": '[{"id":"legacy"}]',
    "buzz-active-workspace-id": "legacy",
  });

  const result = ensureLocalOnlyCommunityStorage(storage, "pubkey");

  assert.equal(result?.community.id, "local");
  assert.deepEqual(
    result?.removedCommunities.map((community) => community.id),
    ["remote"],
  );
  assert.deepEqual(JSON.parse(storage.getItem("buzz-communities")), [
    {
      id: "local",
      name: "Local Dev",
      relayUrl: "ws://localhost:3000",
      pubkey: "pubkey",
      addedAt: "2026-01-01T00:00:00.000Z",
    },
  ]);
  assert.equal(storage.getItem("buzz-active-community-id"), "local");
  assert.equal(storage.getItem("buzz-workspaces"), null);
  assert.equal(storage.getItem("buzz-active-workspace-id"), null);
});

test("failed active-id write rolls back the community purge", () => {
  const originalCommunities = JSON.stringify([
    {
      id: "remote",
      name: "Remote",
      relayUrl: "wss://relay.example.com",
      addedAt: "2026-01-01T00:00:00.000Z",
    },
  ]);
  const storage = createMemoryStorage({
    "buzz-communities": originalCommunities,
    "buzz-active-community-id": "remote",
    "buzz-workspaces": '[{"id":"legacy"}]',
    "buzz-active-workspace-id": "legacy",
  });
  const baseSetItem = storage.setItem;
  storage.setItem = (key, value) => {
    if (key === "buzz-active-community-id") {
      throw new Error("QuotaExceededError");
    }
    baseSetItem(key, value);
  };

  assert.equal(ensureLocalOnlyCommunityStorage(storage, "pubkey"), null);
  assert.equal(storage.getItem("buzz-communities"), originalCommunities);
  assert.equal(storage.getItem("buzz-active-community-id"), "remote");
  assert.equal(storage.getItem("buzz-workspaces"), '[{"id":"legacy"}]');
  assert.equal(storage.getItem("buzz-active-workspace-id"), "legacy");
});

test("first-community fallback rejects every non-canonical relay", () => {
  for (const relay of [
    "wss://relay.example.com",
    "ws://127.0.0.1:3000",
    "ws://localhost:3000/",
  ]) {
    assert.equal(initFirstCommunity(relay, "pubkey"), null);
  }
});

test("clearCommunityStorage removes new and legacy state", () => {
  const storage = createMemoryStorage({
    "buzz-communities": "new",
    "buzz-active-community-id": "new",
    "buzz-workspaces": "old",
    "buzz-active-workspace-id": "old",
  });

  clearCommunityStorage(storage);
  migrateLegacyCommunityStorage(storage);

  assert.equal(storage.length, 0);
});
