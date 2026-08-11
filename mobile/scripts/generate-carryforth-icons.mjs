#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync, inflateSync } from "node:zlib";

const mode = process.argv[2] ?? "--check";
if (!new Set(["--check", "--write"]).has(mode)) {
  console.error("usage: node mobile/scripts/generate-carryforth-icons.mjs [--check|--write]");
  process.exit(2);
}

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "../..");
const desktopDir = join(repoRoot, "desktop");
const sourceSvg = join(repoRoot, "desktop/src-tauri/icons/carryforth-source.svg");
const tauriCli = join(desktopDir, "node_modules/@tauri-apps/cli/tauri.js");
const expectedTauriVersion = "tauri-cli 2.11.2";
const carryforthBackground = "#20242B";

if (!existsSync(tauriCli)) {
  console.error("missing pinned Tauri CLI; run `pnpm install --frozen-lockfile` first");
  process.exit(1);
}

function runTauri(args) {
  const result = spawnSync(process.execPath, [tauriCli, ...args], {
    cwd: desktopDir,
    encoding: "utf8",
    env: process.env,
  });
  if (result.status !== 0) {
    process.stderr.write(result.stdout ?? "");
    process.stderr.write(result.stderr ?? "");
    process.exit(result.status ?? 1);
  }
  return (result.stdout ?? "").trim();
}

const actualTauriVersion = runTauri(["--version"]);
if (actualTauriVersion !== expectedTauriVersion) {
  console.error(
    `unexpected Tauri CLI version: ${actualTauriVersion}; expected ${expectedTauriVersion}`,
  );
  process.exit(1);
}

const tempRoot = mkdtempSync(join(tmpdir(), "carryforth-mobile-icons-"));
const inputDir = join(tempRoot, "input");
const standardDir = join(tempRoot, "standard");
const legacyDir = join(tempRoot, "legacy");
mkdirSync(inputDir, { recursive: true });
copyFileSync(sourceSvg, join(inputDir, "carryforth.svg"));
writeFileSync(
  join(inputDir, "manifest.json"),
  `${JSON.stringify(
    { default: "carryforth.svg", bg_color: carryforthBackground },
    null,
    2,
  )}\n`,
);

try {
  runTauri(["icon", join(inputDir, "manifest.json"), "-o", standardDir]);
  runTauri([
    "icon",
    sourceSvg,
    "-o",
    legacyDir,
    "-p",
    "48,72,96,144,192",
  ]);

  const androidDensities = [
    ["mdpi", 48, 108],
    ["hdpi", 72, 162],
    ["xhdpi", 96, 216],
    ["xxhdpi", 144, 324],
    ["xxxhdpi", 192, 432],
  ];
  const renditions = [
    {
      generated: sourceSvg,
      target: join(repoRoot, "mobile/assets/images/carryforth.svg"),
      kind: "svg",
    },
    {
      generated: join(standardDir, "android/mipmap-anydpi-v26/ic_launcher.xml"),
      target: join(
        repoRoot,
        "mobile/android/app/src/main/res/mipmap-anydpi-v26/ic_launcher.xml",
      ),
      kind: "text",
    },
    {
      generated: join(standardDir, "android/values/ic_launcher_background.xml"),
      target: join(
        repoRoot,
        "mobile/android/app/src/main/res/values/ic_launcher_background.xml",
      ),
      kind: "text",
    },
  ];

  for (const [density, launcherSize, foregroundSize] of androidDensities) {
    const targetDir = join(
      repoRoot,
      `mobile/android/app/src/main/res/mipmap-${density}`,
    );
    const launcher = join(legacyDir, `${launcherSize}x${launcherSize}.png`);
    renditions.push(
      {
        generated: launcher,
        target: join(targetDir, "ic_launcher.png"),
        kind: "png",
        size: launcherSize,
      },
      {
        generated: launcher,
        target: join(targetDir, "ic_launcher_round.png"),
        kind: "png",
        size: launcherSize,
      },
      {
        generated: join(
          standardDir,
          `android/mipmap-${density}/ic_launcher_foreground.png`,
        ),
        target: join(targetDir, "ic_launcher_foreground.png"),
        kind: "png",
        size: foregroundSize,
      },
    );
  }

  const iosMappings = [
    ["AppIcon-20x20@1x.png", "Icon-App-20x20@1x.png", 20],
    ["AppIcon-20x20@2x.png", "Icon-App-20x20@2x.png", 40],
    ["AppIcon-20x20@3x.png", "Icon-App-20x20@3x.png", 60],
    ["AppIcon-29x29@1x.png", "Icon-App-29x29@1x.png", 29],
    ["AppIcon-29x29@2x.png", "Icon-App-29x29@2x.png", 58],
    ["AppIcon-29x29@3x.png", "Icon-App-29x29@3x.png", 87],
    ["AppIcon-40x40@1x.png", "Icon-App-40x40@1x.png", 40],
    ["AppIcon-40x40@2x.png", "Icon-App-40x40@2x.png", 80],
    ["AppIcon-40x40@3x.png", "Icon-App-40x40@3x.png", 120],
    ["AppIcon-60x60@2x.png", "Icon-App-60x60@2x.png", 120],
    ["AppIcon-60x60@3x.png", "Icon-App-60x60@3x.png", 180],
    ["AppIcon-76x76@1x.png", "Icon-App-76x76@1x.png", 76],
    ["AppIcon-76x76@2x.png", "Icon-App-76x76@2x.png", 152],
    ["AppIcon-83.5x83.5@2x.png", "Icon-App-83.5x83.5@2x.png", 167],
    ["AppIcon-512@2x.png", "Icon-App-1024x1024@1x.png", 1024],
  ];
  // Tauri renders SVGs to RGBA. The AppIcon catalog must be opaque, so the
  // write/check contract composites every iOS rendition onto the declared
  // Carryforth background and emits a deterministic RGB PNG.
  for (const [generatedName, targetName, size] of iosMappings) {
    renditions.push({
      generated: join(standardDir, "ios", generatedName),
      target: join(
        repoRoot,
        "mobile/ios/Runner/Assets.xcassets/AppIcon.appiconset",
        targetName,
      ),
      kind: "ios-png",
      size,
    });
  }

  const stalePaths = [
    "mobile/assets/images/buzz-icon.png",
    "mobile/assets/fonts/Geist-Variable.ttf",
    "mobile/assets/fonts/Geist-Italic-Variable.ttf",
    "mobile/assets/fonts/GeistMono-Variable.ttf",
    "mobile/assets/fonts/GeistMono-Italic-Variable.ttf",
    "mobile/ios/Runner/Assets.xcassets/LaunchImage.imageset",
    ...androidDensities.map(
      ([density]) =>
        `mobile/android/app/src/main/res/mipmap-${density}/launch_image.png`,
    ),
  ];

  const failures = [];
  for (const rendition of renditions) {
    const sourceBytes = readFileSync(rendition.generated);
    const generatedBytes =
      rendition.kind === "ios-png"
        ? flattenPngToRgb(sourceBytes, [32, 36, 43])
        : sourceBytes;
    if (rendition.kind === "png" || rendition.kind === "ios-png") {
      const { width, height } = pngDimensions(generatedBytes);
      if (width !== rendition.size || height !== rendition.size) {
        failures.push(
          `generator produced ${width}x${height}, expected ${rendition.size}x${rendition.size}: ${rendition.generated}`,
        );
        continue;
      }
    }
    if (mode === "--write") {
      mkdirSync(dirname(rendition.target), { recursive: true });
      writeFileSync(rendition.target, generatedBytes);
      continue;
    }
    if (!existsSync(rendition.target)) {
      failures.push(`missing rendition: ${rendition.target}`);
      continue;
    }
    const targetBytes = readFileSync(rendition.target);
    if (!generatedBytes.equals(targetBytes)) {
      failures.push(`stale rendition: ${rendition.target}`);
    }
  }

  const contentsPath = join(
    repoRoot,
    "mobile/ios/Runner/Assets.xcassets/AppIcon.appiconset/Contents.json",
  );
  const contents = JSON.parse(readFileSync(contentsPath, "utf8"));
  for (const image of contents.images ?? []) {
    if (!image.filename) continue;
    const path = join(dirname(contentsPath), image.filename);
    if (!existsSync(path)) failures.push(`AppIcon catalog references missing file: ${path}`);
  }

  assertText(
    join(repoRoot, "mobile/android/app/src/main/AndroidManifest.xml"),
    ['android:icon="@mipmap/ic_launcher"'],
    ["launch_image"],
    failures,
  );
  for (const launchBackground of [
    "mobile/android/app/src/main/res/drawable/launch_background.xml",
    "mobile/android/app/src/main/res/drawable-v21/launch_background.xml",
  ]) {
    assertText(
      join(repoRoot, launchBackground),
      ['android:drawable="@color/ic_launcher_background"'],
      ["<bitmap", "launch_image"],
      failures,
    );
  }
  assertText(
    join(repoRoot, "mobile/ios/Runner/Base.lproj/LaunchScreen.storyboard"),
    ['red="0.1254901961" green="0.1411764706" blue="0.168627451"'],
    ["LaunchImage", "<imageView"],
    failures,
  );
  assertText(
    join(repoRoot, "mobile/pubspec.yaml"),
    ["assets/images/carryforth.svg"],
    ["Geist", "buzz-icon"],
    failures,
  );

  for (const relativePath of stalePaths) {
    const path = join(repoRoot, relativePath);
    if (existsSync(path)) failures.push(`retired mobile asset remains: ${relativePath}`);
  }

  if (failures.length > 0) {
    console.error(failures.join("\n"));
    if (mode === "--check") {
      console.error(
        "run `node mobile/scripts/generate-carryforth-icons.mjs --write` after removing retired assets",
      );
    }
    process.exit(1);
  }

  console.log(
    mode === "--write"
      ? `wrote ${renditions.length} Carryforth mobile icon renditions`
      : `verified ${renditions.length} Carryforth mobile icon renditions`,
  );
} finally {
  rmSync(tempRoot, { recursive: true, force: true });
}

function pngDimensions(bytes) {
  const signature = "89504e470d0a1a0a";
  if (bytes.length < 24 || bytes.subarray(0, 8).toString("hex") !== signature) {
    throw new Error("not a PNG file");
  }
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

function flattenPngToRgb(bytes, background) {
  const chunks = parsePng(bytes);
  const ihdr = chunks.find((chunk) => chunk.type === "IHDR")?.payload;
  if (!ihdr || ihdr.length !== 13) throw new Error("PNG is missing IHDR");
  const width = ihdr.readUInt32BE(0);
  const height = ihdr.readUInt32BE(4);
  if (
    ihdr[8] !== 8 ||
    ihdr[9] !== 6 ||
    ihdr[10] !== 0 ||
    ihdr[11] !== 0 ||
    ihdr[12] !== 0
  ) {
    throw new Error("expected an 8-bit, non-interlaced RGBA PNG rendition");
  }

  const compressed = Buffer.concat(
    chunks.filter((chunk) => chunk.type === "IDAT").map((chunk) => chunk.payload),
  );
  const rgba = unfilterPng(inflateSync(compressed), width, height, 4);
  const scanlines = Buffer.alloc(height * (1 + width * 3));
  let sourceOffset = 0;
  let outputOffset = 0;
  for (let y = 0; y < height; y += 1) {
    scanlines[outputOffset] = 0;
    outputOffset += 1;
    for (let x = 0; x < width; x += 1) {
      const alpha = rgba[sourceOffset + 3];
      for (let channel = 0; channel < 3; channel += 1) {
        const source = rgba[sourceOffset + channel];
        scanlines[outputOffset + channel] = Math.floor(
          (source * alpha + background[channel] * (255 - alpha) + 127) / 255,
        );
      }
      sourceOffset += 4;
      outputOffset += 3;
    }
  }

  const outputIhdr = Buffer.from(ihdr);
  outputIhdr[9] = 2;
  return Buffer.concat([
    Buffer.from("89504e470d0a1a0a", "hex"),
    pngChunk("IHDR", outputIhdr),
    pngChunk("IDAT", deflateSync(scanlines, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function parsePng(bytes) {
  if (bytes.subarray(0, 8).toString("hex") !== "89504e470d0a1a0a") {
    throw new Error("not a PNG file");
  }
  const chunks = [];
  let offset = 8;
  while (offset < bytes.length) {
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString("ascii", offset + 4, offset + 8);
    chunks.push({ type, payload: bytes.subarray(offset + 8, offset + 8 + length) });
    offset += length + 12;
  }
  return chunks;
}

function unfilterPng(scanlines, width, height, bytesPerPixel) {
  const rowLength = width * bytesPerPixel;
  const expectedLength = height * (rowLength + 1);
  if (scanlines.length !== expectedLength) {
    throw new Error(`unexpected PNG scanline length: ${scanlines.length}`);
  }
  const output = Buffer.alloc(width * height * bytesPerPixel);
  let inputOffset = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = scanlines[inputOffset];
    inputOffset += 1;
    const rowOffset = y * rowLength;
    for (let x = 0; x < rowLength; x += 1) {
      const left = x >= bytesPerPixel ? output[rowOffset + x - bytesPerPixel] : 0;
      const above = y > 0 ? output[rowOffset + x - rowLength] : 0;
      const upperLeft =
        y > 0 && x >= bytesPerPixel
          ? output[rowOffset + x - rowLength - bytesPerPixel]
          : 0;
      const predictor = switchPngFilter(filter, left, above, upperLeft);
      output[rowOffset + x] = (scanlines[inputOffset] + predictor) & 0xff;
      inputOffset += 1;
    }
  }
  return output;
}

function switchPngFilter(filter, left, above, upperLeft) {
  if (filter === 0) return 0;
  if (filter === 1) return left;
  if (filter === 2) return above;
  if (filter === 3) return Math.floor((left + above) / 2);
  if (filter === 4) return paeth(left, above, upperLeft);
  throw new Error(`unsupported PNG filter: ${filter}`);
}

function paeth(left, above, upperLeft) {
  const estimate = left + above - upperLeft;
  const leftDistance = Math.abs(estimate - left);
  const aboveDistance = Math.abs(estimate - above);
  const upperLeftDistance = Math.abs(estimate - upperLeft);
  if (leftDistance <= aboveDistance && leftDistance <= upperLeftDistance) return left;
  if (aboveDistance <= upperLeftDistance) return above;
  return upperLeft;
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

function assertText(path, required, forbidden, failures) {
  const content = readFileSync(path, "utf8");
  for (const expected of required) {
    if (!content.includes(expected)) failures.push(`missing ${expected} in ${path}`);
  }
  for (const retired of forbidden) {
    if (content.includes(retired)) failures.push(`retired ${retired} remains in ${path}`);
  }
}
