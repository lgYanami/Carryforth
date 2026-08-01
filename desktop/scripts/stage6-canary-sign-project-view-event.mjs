#!/usr/bin/env node

import { readFile, writeFile } from "node:fs/promises";

import { finalizeEvent } from "nostr-tools/pure";

const [commandPath, outputPath] = process.argv.slice(2);
const privateKey = process.env.BUZZ_PRIVATE_KEY?.trim();

if (!commandPath || !outputPath || !privateKey) {
  console.error(
    "usage: BUZZ_PRIVATE_KEY=<hex> stage6-canary-sign-project-view-event.mjs <command.json> <event.json>",
  );
  process.exit(2);
}
if (!/^[0-9a-f]{64}$/i.test(privateKey)) {
  console.error("BUZZ_PRIVATE_KEY must be a 32-byte hex private key");
  process.exit(2);
}

const command = JSON.parse(await readFile(commandPath, "utf8"));
const event = finalizeEvent(
  {
    kind: 44300,
    created_at: Math.floor(Date.now() / 1000),
    tags: [["-"], ["t", "buzz-project-view-mutation"]],
    content: JSON.stringify(command),
  },
  Uint8Array.from(Buffer.from(privateKey, "hex")),
);
await writeFile(outputPath, `${JSON.stringify(event)}\n`, { mode: 0o600 });
