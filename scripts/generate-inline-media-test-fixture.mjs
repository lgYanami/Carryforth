#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const PIXEL_RGBA = Buffer.from([46, 204, 143, 255]);
const EXPECTED_SHA256 =
  "fae7190ceb56f6d7872f9272c2cc40cb7b35c3eee1d92844517ba7873eb7bafa";
const FIXTURE_PATHS = [
  "desktop/src/features/agents/ui/agentSessionToolItemHelpers.test.mjs",
  "desktop/src/features/profile/lib/selfProfileStorage.test.mjs",
  "desktop/tests/e2e/onboarding.spec.ts",
  "desktop/tests/e2e/profile-custom-emoji-status.spec.ts",
  "desktop/tests/e2e/relay-connectivity.spec.ts",
  "mobile/test/shared/relay/media_image_test.dart",
  "mobile/test/shared/widgets/avatar_image_test.dart",
];

function uint32(value) {
  const bytes = Buffer.alloc(4);
  bytes.writeUInt32BE(value >>> 0);
  return bytes;
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

function adler32(bytes) {
  let first = 1;
  let second = 0;
  for (const byte of bytes) {
    first = (first + byte) % 65521;
    second = (second + first) % 65521;
  }
  return ((second << 16) | first) >>> 0;
}

function chunk(kind, data) {
  const type = Buffer.from(kind, "ascii");
  return Buffer.concat([
    uint32(data.length),
    type,
    data,
    uint32(crc32(Buffer.concat([type, data]))),
  ]);
}

export function generateInlineMediaTestPng() {
  const ihdr = Buffer.concat([
    uint32(1),
    uint32(1),
    Buffer.from([8, 6, 0, 0, 0]),
  ]);
  const scanline = Buffer.concat([Buffer.from([0]), PIXEL_RGBA]);
  // A zlib stream containing one uncompressed final DEFLATE block. Keeping the
  // stream construction here avoids compressor-version-dependent output.
  const idat = Buffer.concat([
    Buffer.from([0x78, 0x01, 0x01, 0x05, 0x00, 0xfa, 0xff]),
    scanline,
    uint32(adler32(scanline)),
  ]);
  return Buffer.concat([
    PNG_SIGNATURE,
    chunk("IHDR", ihdr),
    chunk("IDAT", idat),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const png = generateInlineMediaTestPng();
const sha256 = createHash("sha256").update(png).digest("hex");
if (sha256 !== EXPECTED_SHA256) {
  throw new Error(
    `fixture hash drift: expected ${EXPECTED_SHA256}, got ${sha256}`,
  );
}
const dataUrl = `data:image/png;base64,${png.toString("base64")}`;

if (process.argv.includes("--check")) {
  const root = fileURLToPath(new URL("..", import.meta.url));
  const sources = new Map(
    FIXTURE_PATHS.map((path) => [
      path,
      readFileSync(`${root}/${path}`, "utf8"),
    ]),
  );
  const missing = FIXTURE_PATHS.filter(
    (path) => !sources.get(path)?.includes(dataUrl),
  );
  if (missing.length > 0) {
    throw new Error(`inline media fixture missing from: ${missing.join(", ")}`);
  }
  const barePngPayloads = [];
  for (const [path, source] of sources) {
    for (const match of source.matchAll(/iVBORw0KGgo[A-Za-z0-9+/=]*/g)) {
      const prefix = source.slice(0, match.index);
      if (!prefix.endsWith("data:image/png;base64,")) {
        barePngPayloads.push(`${path}:${match.index}`);
      }
    }
  }
  if (barePngPayloads.length > 0) {
    throw new Error(`bare PNG payloads remain: ${barePngPayloads.join(", ")}`);
  }
  console.log(
    `inline media test fixture verified (${png.length} bytes, ${sha256})`,
  );
} else {
  console.log(dataUrl);
}
