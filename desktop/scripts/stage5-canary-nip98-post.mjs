#!/usr/bin/env node

import { createHash, randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";

import { finalizeEvent } from "nostr-tools/pure";

const [url, bodyPath] = process.argv.slice(2);
const privateKey = process.env.BUZZ_PRIVATE_KEY?.trim();

if (!url || !bodyPath || !privateKey) {
  console.error(
    "usage: BUZZ_PRIVATE_KEY=<hex> stage5-canary-nip98-post.mjs <url> <body.json>",
  );
  process.exit(2);
}
if (!/^[0-9a-f]{64}$/i.test(privateKey)) {
  console.error("BUZZ_PRIVATE_KEY must be a 32-byte hex private key");
  process.exit(2);
}

const body = await readFile(bodyPath);
const event = finalizeEvent(
  {
    kind: 27235,
    created_at: Math.floor(Date.now() / 1000),
    tags: [
      ["u", url],
      ["method", "POST"],
      ["payload", createHash("sha256").update(body).digest("hex")],
      ["nonce", randomBytes(16).toString("hex")],
    ],
    content: "",
  },
  Uint8Array.from(Buffer.from(privateKey, "hex")),
);
const authorization = Buffer.from(JSON.stringify(event)).toString("base64");
const response = await fetch(url, {
  method: "POST",
  headers: {
    Authorization: `Nostr ${authorization}`,
    "Content-Type": "application/json",
  },
  body,
});
const responseBody = await response.text();
process.stdout.write(responseBody);
if (!responseBody.endsWith("\n")) {
  process.stdout.write("\n");
}
if (!response.ok) {
  console.error(`operator request failed with HTTP ${response.status}`);
  process.exit(1);
}
