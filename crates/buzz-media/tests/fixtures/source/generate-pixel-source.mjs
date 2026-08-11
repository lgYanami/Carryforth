#!/usr/bin/env node

import { deflateSync } from "node:zlib";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const mode = process.argv[2] ?? "--check";
if (!new Set(["--check", "--write"]).has(mode)) {
  console.error("usage: node generate-pixel-source.mjs [--check|--write]");
  process.exit(2);
}

const output = join(dirname(fileURLToPath(import.meta.url)), "pixel-grid-2x2.png");
const expected = createPng();

if (mode === "--write") {
  writeFileSync(output, expected);
  console.log(`wrote ${output}`);
} else if (!existsSync(output) || !readFileSync(output).equals(expected)) {
  console.error(`stale generated fixture source: ${output}`);
  process.exit(1);
} else {
  console.log(`verified ${output}`);
}

function createPng() {
  // Two red pixels followed by two green pixels. Every byte is defined here;
  // the source is synthetic and contains no external image material.
  const scanlines = Buffer.from([
    0,
    255,
    0,
    0,
    255,
    0,
    0,
    0,
    0,
    255,
    0,
    0,
    255,
    0,
  ]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(2, 0);
  ihdr.writeUInt32BE(2, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;

  return Buffer.concat([
    Buffer.from("89504e470d0a1a0a", "hex"),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", deflateSync(scanlines, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function pngChunk(type, payload) {
  const typeBytes = Buffer.from(type, "ascii");
  const chunk = Buffer.alloc(12 + payload.length);
  chunk.writeUInt32BE(payload.length, 0);
  typeBytes.copy(chunk, 4);
  payload.copy(chunk, 8);
  chunk.writeUInt32BE(crc32(Buffer.concat([typeBytes, payload])), 8 + payload.length);
  return chunk;
}

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}
