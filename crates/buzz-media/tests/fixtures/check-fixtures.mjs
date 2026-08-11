#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";

const fixtureRoot = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(fixtureRoot, "../../../..");
const manifest = JSON.parse(
  readFileSync(join(fixtureRoot, "fixture-manifest.json"), "utf8"),
);
const failures = [];

const generator = join(fixtureRoot, manifest.source.generator);
const generatedSourceCheck = spawnSync(process.execPath, [generator, "--check"], {
  encoding: "utf8",
});
if (generatedSourceCheck.status !== 0) {
  failures.push(
    `source generator check failed:\n${generatedSourceCheck.stdout}${generatedSourceCheck.stderr}`,
  );
}

const expectedFiles = new Map([
  [manifest.source.path, manifest.source.sha256],
  ...Object.entries(manifest.android.files),
  ...Object.entries(manifest.ios.files),
]);
const discoveredCanonicalMedia = walkMedia(fixtureRoot).map((path) =>
  path.slice(fixtureRoot.length + 1).replaceAll("\\", "/"),
);
for (const relativePath of discoveredCanonicalMedia) {
  if (!expectedFiles.has(relativePath)) {
    failures.push(`canonical media is missing from fixture manifest: ${relativePath}`);
  }
}
for (const relativePath of expectedFiles.keys()) {
  if (!discoveredCanonicalMedia.includes(relativePath)) {
    failures.push(`fixture manifest points to missing media: ${relativePath}`);
  }
}

for (const generatorPath of [
  manifest.source.generator,
  manifest.android.generator,
  manifest.ios.generator,
]) {
  if (!existsSync(join(fixtureRoot, generatorPath))) {
    failures.push(`missing tracked fixture generator: ${generatorPath}`);
  }
}

for (const [relativePath, expectedHash] of expectedFiles) {
  const path = join(fixtureRoot, relativePath);
  if (!existsSync(path)) {
    failures.push(`missing canonical fixture: ${relativePath}`);
    continue;
  }
  const actualHash = sha256(readFileSync(path));
  if (actualHash !== expectedHash) {
    failures.push(
      `fixture hash mismatch: ${relativePath}\n  expected ${expectedHash}\n  actual   ${actualHash}`,
    );
  }
}

const expectedCopies = new Set(
  manifest.duplicate_contracts.flatMap((contract) => contract.copies),
);
for (const copyRoot of [
  "mobile/android/app/src/test/resources/fixtures",
  "mobile/android/app/src/androidTest/resources/fixtures",
  "mobile/ios/RunnerTests/Fixtures",
]) {
  for (const path of walkMedia(join(repoRoot, copyRoot))) {
    const relativePath = path.slice(repoRoot.length + 1).replaceAll("\\", "/");
    if (!expectedCopies.has(relativePath)) {
      failures.push(`mobile fixture copy is missing from duplicate contract: ${relativePath}`);
    }
  }
}

for (const contract of manifest.duplicate_contracts) {
  const canonical = readFileSync(join(fixtureRoot, contract.canonical));
  for (const copyPath of contract.copies) {
    const copy = join(repoRoot, copyPath);
    if (!existsSync(copy)) {
      failures.push(`missing fixture copy: ${copyPath}`);
    } else if (!readFileSync(copy).equals(canonical)) {
      failures.push(`fixture copy diverged from ${contract.canonical}: ${copyPath}`);
    }
  }
}

for (const equalPaths of manifest.intentional_equal_hashes) {
  const [first, ...rest] = equalPaths.map((path) => readFileSync(join(fixtureRoot, path)));
  if (!rest.every((bytes) => bytes.equals(first))) {
    failures.push(`intentional equal-hash contract diverged: ${equalPaths.join(", ")}`);
  }
}


const allowedEqualGroups = new Set(
  manifest.intentional_equal_hashes.map((paths) => [...paths].sort().join("\n")),
);
const pathsByHash = new Map();
for (const [path] of expectedFiles) {
  const hash = sha256(readFileSync(join(fixtureRoot, path)));
  const paths = pathsByHash.get(hash) ?? [];
  paths.push(path);
  pathsByHash.set(hash, paths);
}
for (const paths of pathsByHash.values()) {
  if (paths.length < 2) continue;
  const key = [...paths].sort().join("\n");
  if (!allowedEqualGroups.has(key)) {
    failures.push(`unclassified equal-hash canonical fixtures: ${paths.join(", ")}`);
  }
}

const sourcePixels = pngPixels(
  readFileSync(join(fixtureRoot, manifest.source.path)),
);
for (const path of ["ios/uikit-encoded.png", "ios/uikit-sanitized.png"]) {
  const encodedPixels = pngPixels(readFileSync(join(fixtureRoot, path)));
  if (!encodedPixels.equals(sourcePixels)) {
    failures.push(`UIKit PNG pixels diverged from synthetic source: ${path}`);
  }
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exit(1);
}

console.log(
  `verified ${expectedFiles.size} canonical fixtures and ${manifest.duplicate_contracts.length} copy contracts`,
);

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function pngPixels(bytes) {
  if (bytes.subarray(0, 8).toString("hex") !== "89504e470d0a1a0a") {
    throw new Error("fixture is not PNG");
  }
  const idat = [];
  let width;
  let height;
  let offset = 8;
  while (offset < bytes.length) {
    const size = bytes.readUInt32BE(offset);
    const type = bytes.toString("ascii", offset + 4, offset + 8);
    if (type === "IHDR") {
      width = bytes.readUInt32BE(offset + 8);
      height = bytes.readUInt32BE(offset + 12);
      const ihdr = bytes.subarray(offset + 8, offset + 8 + size);
      if (
        ihdr[8] !== 8 ||
        ihdr[9] !== 2 ||
        ihdr[10] !== 0 ||
        ihdr[11] !== 0 ||
        ihdr[12] !== 0
      ) {
        throw new Error("fixture source must be an 8-bit non-interlaced RGB PNG");
      }
    }
    if (type === "IDAT") idat.push(bytes.subarray(offset + 8, offset + 8 + size));
    offset += size + 12;
  }
  if (!width || !height) throw new Error("fixture PNG is missing IHDR");
  const scanlines = inflateSync(Buffer.concat(idat));
  const bytesPerPixel = 3;
  const rowLength = width * bytesPerPixel;
  if (scanlines.length !== height * (rowLength + 1)) {
    throw new Error("fixture PNG has an unexpected scanline length");
  }
  const pixels = Buffer.alloc(width * height * bytesPerPixel);
  let inputOffset = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = scanlines[inputOffset];
    inputOffset += 1;
    const rowOffset = y * rowLength;
    for (let x = 0; x < rowLength; x += 1) {
      const left = x >= bytesPerPixel ? pixels[rowOffset + x - bytesPerPixel] : 0;
      const above = y > 0 ? pixels[rowOffset + x - rowLength] : 0;
      const upperLeft =
        y > 0 && x >= bytesPerPixel
          ? pixels[rowOffset + x - rowLength - bytesPerPixel]
          : 0;
      pixels[rowOffset + x] =
        (scanlines[inputOffset] + pngPredictor(filter, left, above, upperLeft)) &
        0xff;
      inputOffset += 1;
    }
  }
  return pixels;
}

function pngPredictor(filter, left, above, upperLeft) {
  if (filter === 0) return 0;
  if (filter === 1) return left;
  if (filter === 2) return above;
  if (filter === 3) return Math.floor((left + above) / 2);
  if (filter === 4) {
    const estimate = left + above - upperLeft;
    const leftDistance = Math.abs(estimate - left);
    const aboveDistance = Math.abs(estimate - above);
    const upperLeftDistance = Math.abs(estimate - upperLeft);
    if (leftDistance <= aboveDistance && leftDistance <= upperLeftDistance) return left;
    if (aboveDistance <= upperLeftDistance) return above;
    return upperLeft;
  }
  throw new Error(`unsupported PNG filter: ${filter}`);
}

function walkMedia(root) {
  if (!existsSync(root)) return [];
  const result = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) result.push(...walkMedia(path));
    if (entry.isFile() && /\.(?:jpe?g|png)$/i.test(entry.name)) result.push(path);
  }
  return result.sort();
}
