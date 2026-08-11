#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync, lstatSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "..");
const manifestPath = path.join(repoRoot, "docs/release/source-assets.json");
const reportOnly = process.argv.length === 3 && process.argv[2] === "--report";
if (process.argv.length > (reportOnly ? 3 : 2)) {
  console.error(
    "Usage: node scripts/check-source-asset-inventory.mjs [--report]",
  );
  process.exit(2);
}

const failures = [];
const fail = (message) => failures.push(message);
const sha256 = (value) => createHash("sha256").update(value).digest("hex");

function lstatIfPresent(absolute) {
  try {
    return lstatSync(absolute);
  } catch (error) {
    if (error?.code === "ENOENT") return undefined;
    throw error;
  }
}

function trackedAndUntrackedFiles() {
  const result = spawnSync(
    "git",
    ["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    { cwd: repoRoot, encoding: "buffer" },
  );
  if (result.status !== 0) {
    const stderr = result.stderr?.toString("utf8").trim();
    throw new Error(`git ls-files failed${stderr ? `: ${stderr}` : ""}`);
  }
  return result.stdout.toString("utf8").split("\0").filter(Boolean).sort();
}

const assetExtensions = new Set([
  ".gif",
  ".heic",
  ".heif",
  ".icns",
  ".ico",
  ".jpeg",
  ".jpg",
  ".m4a",
  ".aac",
  ".aif",
  ".aiff",
  ".avif",
  ".bmp",
  ".flac",
  ".mov",
  ".mp3",
  ".mp4",
  ".otf",
  ".ogg",
  ".opus",
  ".pdf",
  ".png",
  ".svg",
  ".ttf",
  ".tif",
  ".tiff",
  ".wav",
  ".webm",
  ".webp",
  ".woff",
  ".woff2",
]);

function detectedAssetType(bytes) {
  if (
    bytes.length >= 8 &&
    bytes.subarray(0, 8).equals(Buffer.from("89504e470d0a1a0a", "hex"))
  )
    return "png";
  if (
    bytes.length >= 3 &&
    bytes[0] === 0xff &&
    bytes[1] === 0xd8 &&
    bytes[2] === 0xff
  )
    return "jpeg";
  if (
    bytes.subarray(0, 6).toString("ascii") === "GIF87a" ||
    bytes.subarray(0, 6).toString("ascii") === "GIF89a"
  )
    return "gif";
  if (
    bytes.length >= 12 &&
    bytes.subarray(0, 4).toString("ascii") === "RIFF" &&
    bytes.subarray(8, 12).toString("ascii") === "WAVE"
  )
    return "wav";
  if (
    bytes.length >= 12 &&
    bytes.subarray(0, 4).toString("ascii") === "RIFF" &&
    bytes.subarray(8, 12).toString("ascii") === "WEBP"
  )
    return "webp";
  if (bytes.length >= 12 && bytes.subarray(4, 8).toString("ascii") === "ftyp")
    return "iso-media";
  if (bytes.subarray(0, 4).toString("ascii") === "fLaC") return "flac";
  if (bytes.subarray(0, 4).toString("ascii") === "OggS") return "ogg";
  if (
    bytes.length >= 2 &&
    bytes[0] === 0xff &&
    (bytes[1] === 0xf1 || bytes[1] === 0xf9)
  )
    return "aac";
  if (
    bytes.length >= 12 &&
    bytes.subarray(0, 4).toString("ascii") === "FORM" &&
    ["AIFF", "AIFC"].includes(bytes.subarray(8, 12).toString("ascii"))
  )
    return "aiff";
  if (
    bytes.length >= 4 &&
    bytes.subarray(0, 4).equals(Buffer.from([0x1a, 0x45, 0xdf, 0xa3]))
  )
    return "ebml-media";
  if (bytes.length >= 2 && bytes.subarray(0, 2).toString("ascii") === "BM")
    return "bmp";
  if (
    bytes.length >= 4 &&
    (bytes.subarray(0, 4).equals(Buffer.from([0x49, 0x49, 0x2a, 0x00])) ||
      bytes.subarray(0, 4).equals(Buffer.from([0x4d, 0x4d, 0x00, 0x2a])))
  )
    return "tiff";
  if (
    bytes.subarray(0, 3).toString("ascii") === "ID3" ||
    (bytes.length >= 2 && bytes[0] === 0xff && (bytes[1] & 0xe0) === 0xe0)
  )
    return "mp3";
  if (
    bytes.length >= 4 &&
    bytes.subarray(0, 4).equals(Buffer.from([0x00, 0x01, 0x00, 0x00]))
  )
    return "ttf";
  if (
    ["OTTO", "wOFF", "wOF2", "icns"].includes(
      bytes.subarray(0, 4).toString("ascii"),
    )
  )
    return bytes.subarray(0, 4).toString("ascii");
  if (
    bytes.length >= 4 &&
    bytes.subarray(0, 4).equals(Buffer.from([0x00, 0x00, 0x01, 0x00]))
  )
    return "ico";
  if (bytes.subarray(0, 5).toString("ascii") === "%PDF-") return "pdf";
  const textHead = bytes
    .subarray(0, 4096)
    .toString("utf8")
    .replace(/^\uFEFF?\s*/, "");
  if (/^(?:<\?xml[^>]*>\s*)?<svg(?:\s|>)/i.test(textHead)) return "svg";
  return undefined;
}

function treeHash(paths) {
  const lines = [...paths]
    .sort()
    .map(
      (relative) =>
        `${sha256(readFileSync(path.join(repoRoot, relative)))}  ${relative}\n`,
    )
    .join("");
  return sha256(lines);
}

function findQuotedEnd(text, start, delimiter) {
  for (let index = start; index < text.length; index += 1) {
    if (text[index] === delimiter && text[index - 1] !== "\\") return index;
  }
  return text.length;
}

function sourceLocator(text, index) {
  const prefix = text.slice(0, index);
  const line = prefix.split("\n").length;
  const lastNewline = prefix.lastIndexOf("\n");
  const column = index - lastNewline;
  return `${line}:${column}`;
}

function decodeDataLiteral(literal) {
  const comma = literal.indexOf(",");
  if (comma < 0) return undefined;
  const metadata = literal.slice(5, comma);
  const [mediaType, ...parameters] = metadata.split(";");
  if (
    !/^(?:(?:image|audio|font|video)\/[a-z0-9.+-]+|application\/(?:pdf|font-[a-z0-9.+-]+|vnd\.ms-fontobject))$/i.test(
      mediaType,
    )
  )
    return undefined;
  const payload = literal.slice(comma + 1);
  const base64 = parameters.some(
    (parameter) => parameter.toLowerCase() === "base64",
  );
  if (base64) {
    const compact = payload
      .replace(/\s+/g, "")
      .replace(/-/g, "+")
      .replace(/_/g, "/");
    if (
      compact.length === 0 ||
      !/^[a-z0-9+/]*={0,2}$/i.test(compact) ||
      /=/.test(compact.slice(0, -2))
    )
      return undefined;
    const padded = compact.padEnd(Math.ceil(compact.length / 4) * 4, "=");
    const decoded = Buffer.from(padded, "base64");
    if (decoded.length === 0) return undefined;
    const canonical = decoded.toString("base64").replace(/=+$/, "");
    if (canonical !== compact.replace(/=+$/, "")) return undefined;
    return {
      bytes: decoded,
      encoding: "base64",
      mediaType: mediaType.toLowerCase(),
    };
  }
  const chunks = [];
  for (let index = 0; index < payload.length; ) {
    if (payload[index] === "%") {
      const hex = payload.slice(index + 1, index + 3);
      if (!/^[0-9a-f]{2}$/i.test(hex)) return undefined;
      chunks.push(Buffer.from([Number.parseInt(hex, 16)]));
      index += 3;
    } else {
      const codePoint = payload.codePointAt(index);
      const character = String.fromCodePoint(codePoint);
      chunks.push(Buffer.from(character, "utf8"));
      index += character.length;
    }
  }
  const decoded = Buffer.concat(chunks);
  if (decoded.length === 0) return undefined;
  if (
    mediaType.toLowerCase() === "image/svg+xml" &&
    !/^\s*(?:<\?xml[^>]*>\s*)?<svg(?:\s|>)/i.test(decoded.toString("utf8"))
  )
    return undefined;
  return {
    bytes: decoded,
    encoding: payload.includes("%") ? "percent" : "raw",
    mediaType: mediaType.toLowerCase(),
  };
}

function embeddedData(files) {
  const found = new Map();
  const markers = new Map();
  const startPattern =
    /data:(?:(?:image|audio|font|video)\/[a-z0-9.+-]+|application\/(?:pdf|font-[a-z0-9.+-]+|vnd\.ms-fontobject))/gi;
  for (const relative of files) {
    const absolute = path.join(repoRoot, relative);
    const bytes = readFileSync(absolute);
    if (bytes.includes(0)) continue;
    const text = bytes.toString("utf8");
    for (const match of text.matchAll(startPattern)) {
      const start = match.index;
      const previous = start > 0 ? text[start - 1] : "";
      let end;
      if (['"', "'", "`"].includes(previous)) {
        end = findQuotedEnd(text, start, previous);
      } else {
        end = start;
        while (end < text.length && !/[\s)<>]/.test(text[end])) end += 1;
      }
      const decoded = decodeDataLiteral(text.slice(start, end));
      const locator = sourceLocator(text, start);
      if (!decoded) {
        const statement = text.slice(start, end);
        if (!statement.includes(",")) continue;
        const statementHash = sha256(statement);
        const statementBytes = Buffer.byteLength(statement, "utf8");
        const signature = [relative, statementHash, statementBytes].join("\0");
        const current = markers.get(signature);
        if (current) {
          current.occurrences += 1;
          current.locators.push(locator);
        } else {
          markers.set(signature, {
            path: relative,
            locators: [locator],
            statement_sha256: statementHash,
            statement_bytes: statementBytes,
            occurrences: 1,
          });
        }
        continue;
      }
      const signature = [
        relative,
        decoded.mediaType,
        decoded.encoding,
        sha256(decoded.bytes),
        decoded.bytes.length,
      ].join("\0");
      const current = found.get(signature);
      if (current) {
        current.occurrences += 1;
        current.locators.push(locator);
      } else {
        found.set(signature, {
          path: relative,
          locators: [locator],
          media_type: decoded.mediaType,
          encoding: decoded.encoding,
          decoded_sha256: sha256(decoded.bytes),
          decoded_bytes: decoded.bytes.length,
          occurrences: 1,
        });
      }
    }
  }
  return {
    decoded: [...found.values()]
      .map((entry) => ({ ...entry, locators: entry.locators.sort() }))
      .sort((left, right) =>
        JSON.stringify(left).localeCompare(JSON.stringify(right)),
      ),
    markers: [...markers.values()]
      .map((entry) => ({ ...entry, locators: entry.locators.sort() }))
      .sort((left, right) =>
        JSON.stringify(left).localeCompare(JSON.stringify(right)),
      ),
  };
}

function bareBase64Media(files) {
  const found = [];
  const quotedBase64 = /(["'`])([a-z0-9+/_-]{16,}={0,2})\1/gi;
  for (const relative of files) {
    const bytes = readFileSync(path.join(repoRoot, relative));
    if (bytes.includes(0)) continue;
    const text = bytes.toString("utf8");
    for (const match of text.matchAll(quotedBase64)) {
      const compact = match[2].replace(/-/g, "+").replace(/_/g, "/");
      if (!/^[a-z0-9+/]*={0,2}$/i.test(compact)) continue;
      const decoded = Buffer.from(
        compact.padEnd(Math.ceil(compact.length / 4) * 4, "="),
        "base64",
      );
      const canonical = decoded.toString("base64").replace(/=+$/, "");
      if (canonical !== compact.replace(/=+$/, "")) continue;
      const detected = detectedAssetType(decoded);
      if (!detected) continue;
      // Headerless MPEG/AAC sync bits are only two bytes and routinely occur
      // when ordinary identifiers happen to be valid URL-safe base64. Keep
      // the bare-literal gate fail closed for strong media signatures, while
      // accepting MP3 only when the decoded literal has an ID3 header.
      if (detected === "aac") continue;
      if (
        detected === "mp3" &&
        decoded.subarray(0, 3).toString("ascii") !== "ID3"
      ) {
        continue;
      }
      found.push({
        path: relative,
        locator: sourceLocator(text, match.index + 1),
        detected_type: detected,
        decoded_sha256: sha256(decoded),
        decoded_bytes: decoded.length,
      });
    }
  }
  return found.sort((left, right) =>
    JSON.stringify(left).localeCompare(JSON.stringify(right)),
  );
}

function completeIsoMediaContainer(bytes) {
  let offset = 0;
  let hasPayloadBox = false;
  while (offset + 8 <= bytes.length) {
    const size = bytes.readUInt32BE(offset);
    if (size < 8 || offset + size > bytes.length) return false;
    const kind = bytes.subarray(offset + 4, offset + 8).toString("ascii");
    if (["mdat", "moov", "meta"].includes(kind)) hasPayloadBox = true;
    offset += size;
  }
  return offset === bytes.length && hasPayloadBox;
}

function numericArrayMedia(files) {
  const found = [];
  const patterns = [
    {
      expression: /(?:0x[0-9a-f]{1,2}\s*,\s*){7,}0x[0-9a-f]{1,2}/gi,
      values: (literal) =>
        [...literal.matchAll(/0x([0-9a-f]{1,2})/gi)].map((match) =>
          Number.parseInt(match[1], 16),
        ),
    },
    {
      expression:
        /(?:\b(?:25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})\s*,\s*){7,}\b(?:25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})/g,
      values: (literal) =>
        [...literal.matchAll(/\b(?:25[0-5]|2[0-4][0-9]|1?[0-9]{1,2})\b/g)].map(
          (match) => Number.parseInt(match[0], 10),
        ),
    },
  ];
  for (const relative of files) {
    const bytes = readFileSync(path.join(repoRoot, relative));
    if (bytes.includes(0)) continue;
    const text = bytes.toString("utf8");
    for (const { expression, values } of patterns) {
      for (const match of text.matchAll(expression)) {
        const decoded = Buffer.from(values(match[0]));
        const detected = detectedAssetType(decoded);
        if (!detected || detected === "aac" || detected === "mp3") continue;
        const completeContainer =
          (detected === "png" && decoded.includes(Buffer.from("IEND"))) ||
          (detected === "jpeg" &&
            decoded.length >= 4 &&
            decoded[decoded.length - 2] === 0xff &&
            decoded[decoded.length - 1] === 0xd9) ||
          (detected === "gif" && decoded[decoded.length - 1] === 0x3b) ||
          (["webp", "wav"].includes(detected) &&
            decoded.length >= 12 &&
            decoded.readUInt32LE(4) + 8 === decoded.length) ||
          (detected === "iso-media" && completeIsoMediaContainer(decoded)) ||
          (!["png", "jpeg", "gif", "webp", "wav", "iso-media"].includes(
            detected,
          ) &&
            decoded.length >= 32);
        if (!completeContainer) continue;
        found.push({
          path: relative,
          locator: sourceLocator(text, match.index),
          detected_type: detected,
          decoded_sha256: sha256(decoded),
          decoded_bytes: decoded.length,
        });
      }
    }
  }
  return found.sort((left, right) =>
    JSON.stringify(left).localeCompare(JSON.stringify(right)),
  );
}

let manifest;
try {
  manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
} catch (error) {
  console.error(`Cannot read source asset inventory: ${error.message}`);
  process.exit(1);
}

const files = trackedAndUntrackedFiles();
for (const relative of files) {
  const absolute = path.join(repoRoot, relative);
  const stat = lstatIfPresent(absolute);
  if (!stat) continue;
  if (
    stat.isSymbolicLink() &&
    assetExtensions.has(path.extname(relative).toLowerCase())
  ) {
    fail(
      `asset-like symlink is not allowed in the source inventory: ${relative}`,
    );
  }
}
const regularFiles = files.filter((relative) => {
  const absolute = path.join(repoRoot, relative);
  const stat = lstatIfPresent(absolute);
  if (!stat) return false;
  return stat.isFile();
});
const regularFileSet = new Set(regularFiles);

const allowedTopLevelKeys = new Set([
  "schema",
  "scope",
  "rights_profiles",
  "marker_profiles",
  "entries",
  "embedded_data",
  "data_uri_markers",
]);
const allowedClassifications = new Set([
  "project-art",
  "project-art-rendition",
  "generated",
  "third-party-font",
  "third-party-media",
  "test-fixture",
]);
const allowedLicenses = new Set(["Apache-2.0", "MIT", "OFL-1.1"]);
const allowedUsage = new Set([
  "source",
  "docs",
  "desktop",
  "mobile",
  "test",
  "package",
]);
const allowedTrademarkStatus = new Set([
  "not-applicable",
  "project-owned",
  "factual-text-only",
  "third-party-cleared",
]);
const allowedMarkerKinds = new Set([
  "runtime-constructor",
  "contract-documentation",
  "test-negative",
  "test-dynamic",
  "docs-placeholder",
]);
const idPattern = /^[a-z0-9][a-z0-9-]*$/;
const shaPattern = /^[a-f0-9]{64}$/;
const commitPattern = /^[a-f0-9]{40}$/;

function rejectUnknownKeys(value, allowed, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
    return;
  }
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) fail(`${label} contains unknown key: ${key}`);
  }
}

function verifyFileEvidence(evidence, label, extraKeys = []) {
  rejectUnknownKeys(evidence, new Set(["path", "sha256", ...extraKeys]), label);
  if (
    !evidence ||
    typeof evidence.path !== "string" ||
    !shaPattern.test(evidence.sha256 ?? "")
  ) {
    fail(`${label} must contain path and lowercase SHA-256`);
    return;
  }
  if (
    path.isAbsolute(evidence.path) ||
    evidence.path.includes("\\") ||
    path.posix.normalize(evidence.path) !== evidence.path ||
    evidence.path.startsWith("../")
  ) {
    fail(
      `${label} path is not a normalized repository-relative POSIX path: ${evidence.path}`,
    );
    return;
  }
  if (!regularFileSet.has(evidence.path)) {
    fail(`${label} does not name a source regular file: ${evidence.path}`);
    return;
  }
  const actual = sha256(readFileSync(path.join(repoRoot, evidence.path)));
  if (actual !== evidence.sha256)
    fail(`${label} SHA-256 is stale: ${evidence.path}`);
}

function validateProvenance(record, label) {
  const provenance = record.provenance;
  const allowedKeys = new Set([
    "description",
    "license_evidence",
    "created_in_commit",
    "source",
    "generator",
    "toolchain",
    "toolchain_evidence",
    "upstream",
    "recipe",
    "reproducible",
  ]);
  rejectUnknownKeys(provenance, allowedKeys, `${label}.provenance`);
  if (
    !provenance ||
    typeof provenance.description !== "string" ||
    provenance.description.trim() === ""
  ) {
    fail(`${label}.provenance requires a description`);
    return;
  }
  if (
    !Array.isArray(provenance.license_evidence) ||
    provenance.license_evidence.length === 0
  ) {
    fail(`${label}.provenance requires local license evidence`);
  } else {
    provenance.license_evidence.forEach((item, index) => {
      verifyFileEvidence(
        item,
        `${label}.provenance.license_evidence[${index}]`,
      );
    });
  }
  if (provenance.source !== undefined) {
    if (!Array.isArray(provenance.source))
      fail(`${label}.provenance.source must be an array`);
    else
      provenance.source.forEach((item, index) => {
        verifyFileEvidence(item, `${label}.provenance.source[${index}]`);
      });
  }
  if (provenance.toolchain_evidence !== undefined) {
    if (!Array.isArray(provenance.toolchain_evidence)) {
      fail(`${label}.provenance.toolchain_evidence must be an array`);
    } else {
      provenance.toolchain_evidence.forEach((item, index) => {
        verifyFileEvidence(
          item,
          `${label}.provenance.toolchain_evidence[${index}]`,
        );
      });
    }
  }
  if (provenance.generator !== undefined) {
    rejectUnknownKeys(
      provenance.generator,
      new Set(["path", "sha256", "command"]),
      `${label}.provenance.generator`,
    );
    verifyFileEvidence(provenance.generator, `${label}.provenance.generator`, [
      "command",
    ]);
    if (
      typeof provenance.generator.command !== "string" ||
      provenance.generator.command.trim() === ""
    ) {
      fail(`${label}.provenance.generator requires an offline command`);
    }
  }
  if (provenance.upstream !== undefined) {
    rejectUnknownKeys(
      provenance.upstream,
      new Set(["url", "ref"]),
      `${label}.provenance.upstream`,
    );
    const immutableRef = provenance.upstream?.ref ?? "";
    const isImmutableRef = /^[a-f0-9]{40}$/.test(immutableRef);
    if (
      !/^https:\/\//.test(provenance.upstream?.url ?? "") ||
      !isImmutableRef
    ) {
      fail(
        `${label}.provenance.upstream requires an HTTPS URL and immutable ref`,
      );
    }
  }
  if (
    provenance.toolchain !== undefined &&
    (typeof provenance.toolchain !== "string" ||
      provenance.toolchain.trim() === "")
  ) {
    fail(`${label}.provenance.toolchain must be a non-empty string`);
  }
  if (
    provenance.recipe !== undefined &&
    (typeof provenance.recipe !== "string" || provenance.recipe.trim() === "")
  ) {
    fail(`${label}.provenance.recipe must be a non-empty string`);
  }
  const classification = record.classification;
  if (classification === "project-art") {
    if (!commitPattern.test(provenance.created_in_commit ?? ""))
      fail(`${label} project art requires created_in_commit`);
  } else if (classification === "project-art-rendition") {
    if (
      !commitPattern.test(provenance.created_in_commit ?? "") ||
      !Array.isArray(provenance.source) ||
      provenance.source.length === 0
    ) {
      fail(
        `${label} project-art-rendition requires source evidence and created_in_commit`,
      );
    }
    if (provenance.reproducible !== false)
      fail(`${label} project-art-rendition must not claim reproducibility`);
  } else if (classification === "generated") {
    if (
      !provenance.generator ||
      !Array.isArray(provenance.source) ||
      provenance.source.length === 0 ||
      typeof provenance.toolchain !== "string" ||
      !Array.isArray(provenance.toolchain_evidence) ||
      provenance.toolchain_evidence.length === 0 ||
      typeof provenance.recipe !== "string"
    ) {
      fail(
        `${label} generated asset requires source, generator, toolchain evidence, and recipe`,
      );
    }
    if (provenance.reproducible !== true)
      fail(`${label} generated asset must declare reproducible=true`);
  } else if (
    classification === "third-party-font" ||
    classification === "third-party-media"
  ) {
    if (!provenance.upstream)
      fail(`${label} third-party asset requires immutable upstream evidence`);
  } else if (classification === "test-fixture") {
    if (!provenance.generator && !provenance.recipe)
      fail(`${label} test fixture requires a generator or explicit recipe`);
    if (
      typeof provenance.toolchain !== "string" ||
      provenance.toolchain.trim() === ""
    )
      fail(`${label} test fixture requires a toolchain`);
  }
}

const seenIds = new Set();
function validateRightsRecord(record, label) {
  if (!idPattern.test(record?.id ?? "")) fail(`${label} has an invalid id`);
  else if (seenIds.has(record.id))
    fail(`duplicate source asset id: ${record.id}`);
  else seenIds.add(record.id);
  if (!allowedClassifications.has(record?.classification))
    fail(`${label} has an invalid classification`);
  if (!allowedLicenses.has(record?.license))
    fail(`${label} has an invalid SPDX license`);
  if (
    !Array.isArray(record?.usage) ||
    record.usage.length === 0 ||
    record.usage.some((usage) => !allowedUsage.has(usage))
  ) {
    fail(`${label} has invalid usage values`);
  }
  if (
    typeof record?.copyright_holder !== "string" ||
    record.copyright_holder.trim() === ""
  )
    fail(`${label} lacks copyright_holder`);
  if (record?.status !== "cleared") fail(`${label} is not cleared`);
  if (!allowedTrademarkStatus.has(record?.trademark_status))
    fail(`${label} has invalid trademark_status`);
  validateProvenance(record, label);
}

rejectUnknownKeys(manifest, allowedTopLevelKeys, "manifest");
if (manifest.schema !== "carryforth.source-assets/v1")
  fail("unexpected manifest schema");
if (typeof manifest.scope !== "string" || manifest.scope.trim() === "")
  fail("manifest scope is required");
if (
  !manifest.rights_profiles ||
  typeof manifest.rights_profiles !== "object" ||
  Array.isArray(manifest.rights_profiles)
) {
  fail("manifest rights_profiles must be an object");
}
if (
  !manifest.marker_profiles ||
  typeof manifest.marker_profiles !== "object" ||
  Array.isArray(manifest.marker_profiles)
) {
  fail("manifest marker_profiles must be an object");
}
if (!Array.isArray(manifest.entries)) fail("manifest entries must be an array");
if (!Array.isArray(manifest.embedded_data))
  fail("manifest embedded_data must be an array");
if (!Array.isArray(manifest.data_uri_markers))
  fail("manifest data_uri_markers must be an array");

const candidates = regularFiles.filter((relative) => {
  const extension = path.extname(relative).toLowerCase();
  return (
    assetExtensions.has(extension) ||
    detectedAssetType(readFileSync(path.join(repoRoot, relative)))
  );
});
const discoveredEmbedded = embeddedData(regularFiles);
const discoveredBareBase64Media = bareBase64Media(regularFiles);
const discoveredNumericArrayMedia = numericArrayMedia(regularFiles);
if (reportOnly) {
  console.log(
    JSON.stringify(
      {
        candidates,
        ...discoveredEmbedded,
        bare_base64_media: discoveredBareBase64Media,
        numeric_array_media: discoveredNumericArrayMedia,
      },
      null,
      2,
    ),
  );
  process.exit(0);
}
for (const bare of discoveredBareBase64Media) {
  fail(
    `bare base64 media bytes must be expressed as a complete inventoried data URI: ${bare.path}:${bare.locator} type=${bare.detected_type} sha256=${bare.decoded_sha256} bytes=${bare.decoded_bytes}`,
  );
}
for (const numeric of discoveredNumericArrayMedia) {
  fail(
    `inline numeric media bytes must be generated at test runtime or stored as an inventoried file: ${numeric.path}:${numeric.locator} type=${numeric.detected_type} sha256=${numeric.decoded_sha256} bytes=${numeric.decoded_bytes}`,
  );
}
const candidateSet = new Set(candidates);
const claimedPaths = new Map();
const entriesById = new Map();

for (const entry of manifest.entries ?? []) {
  rejectUnknownKeys(
    entry,
    new Set([
      "id",
      "classification",
      "license",
      "usage",
      "copyright_holder",
      "status",
      "trademark_status",
      "paths",
      "file_count",
      "tree_sha256",
      "declared_fonts",
      "privacy_review",
      "provenance",
    ]),
    `asset entry ${entry.id ?? "<missing-id>"}`,
  );
  validateRightsRecord(entry, `asset entry ${entry.id ?? "<missing-id>"}`);
  if (!Array.isArray(entry.paths) || entry.paths.length === 0) {
    fail(`asset entry ${entry.id ?? "<missing-id>"} has no paths`);
    continue;
  }
  if (entriesById.has(entry.id)) fail(`duplicate asset entry id: ${entry.id}`);
  entriesById.set(entry.id, entry);
  if (entry.file_count !== entry.paths.length)
    fail(`asset entry ${entry.id} file_count does not match paths`);
  if (!shaPattern.test(entry.tree_sha256 ?? ""))
    fail(`asset entry ${entry.id} has an invalid tree_sha256`);
  if (
    entry.declared_fonts !== undefined &&
    (!Array.isArray(entry.declared_fonts) ||
      entry.declared_fonts.some(
        (font) => typeof font !== "string" || font === "",
      ))
  ) {
    fail(`asset entry ${entry.id} has invalid declared_fonts`);
  }
  const screenshotLike = entry.paths.some((relative) =>
    /(?:^|[._/-])screenshots?(?:[._/-]|$)/i.test(relative),
  );
  if (entry.privacy_review !== undefined) {
    rejectUnknownKeys(
      entry.privacy_review,
      new Set(["mock_only", "no_real_people", "notes"]),
      `asset entry ${entry.id}.privacy_review`,
    );
    if (
      entry.privacy_review.mock_only !== true ||
      entry.privacy_review.no_real_people !== true ||
      typeof entry.privacy_review.notes !== "string" ||
      entry.privacy_review.notes.trim() === ""
    ) {
      fail(`asset entry ${entry.id} has an incomplete privacy_review`);
    }
  }
  if (screenshotLike && entry.privacy_review === undefined) {
    fail(`screenshot-like asset entry ${entry.id} requires privacy_review`);
  }
  let pathsAreReadableAssets = true;
  for (const relative of entry.paths) {
    if (
      path.isAbsolute(relative) ||
      relative.includes("\\") ||
      path.posix.normalize(relative) !== relative ||
      relative.startsWith("../")
    ) {
      fail(`asset entry ${entry.id} path is not normalized: ${relative}`);
    }
    if (!candidateSet.has(relative))
      fail(
        `asset entry ${entry.id} names a missing or non-asset path: ${relative}`,
      );
    if (!candidateSet.has(relative)) pathsAreReadableAssets = false;
    const owner = claimedPaths.get(relative);
    if (owner)
      fail(
        `asset path is claimed by both ${owner} and ${entry.id}: ${relative}`,
      );
    claimedPaths.set(relative, entry.id);
  }
  if (pathsAreReadableAssets && entry.tree_sha256 !== treeHash(entry.paths))
    fail(`asset entry ${entry.id} tree_sha256 is stale`);
}

for (const relative of candidates) {
  if (!claimedPaths.has(relative)) fail(`unlisted source asset: ${relative}`);
}

for (const relative of candidates.filter((candidate) =>
  candidate.toLowerCase().endsWith(".svg"),
)) {
  const owner = claimedPaths.get(relative);
  if (!owner) continue;
  const entry = entriesById.get(owner);
  const text = readFileSync(path.join(repoRoot, relative), "utf8");
  if (/<!DOCTYPE|<!ENTITY/i.test(text))
    fail(`SVG ${relative} contains a forbidden DOCTYPE or entity`);
  if (/@import\b/i.test(text))
    fail(`SVG ${relative} contains a forbidden CSS import`);
  for (const match of text.matchAll(
    /(?:href|xlink:href)\s*=\s*["']([^"']+)["']/gi,
  )) {
    const target = match[1];
    if (!target.startsWith("#"))
      fail(`SVG ${relative} contains a non-local href: ${target.slice(0, 80)}`);
  }
  for (const match of text.matchAll(/url\(\s*["']?([^"')]+)["']?\s*\)/gi)) {
    const target = match[1].trim();
    if (!target.startsWith("#"))
      fail(
        `SVG ${relative} contains a non-local CSS URL: ${target.slice(0, 80)}`,
      );
  }
  if (/<image(?:\s|>)/i.test(text))
    fail(`SVG ${relative} embeds an image element`);
  const actualFonts = new Set();
  for (const match of text.matchAll(
    /font-family\s*=\s*["']([^"']+)["']|font-family\s*:\s*([^;"'}]+)/gi,
  )) {
    const value = (match[1] ?? match[2]).trim();
    for (const font of value.split(","))
      actualFonts.add(font.trim().replace(/^['"]|['"]$/g, ""));
  }
  const declaredFonts = new Set(entry.declared_fonts ?? []);
  for (const font of actualFonts) {
    if (!declaredFonts.has(font))
      fail(`SVG ${relative} uses undeclared font family: ${font}`);
  }
  for (const font of declaredFonts) {
    if (
      ![...entry.paths].some(
        (assetPath) =>
          assetPath.toLowerCase().endsWith(".svg") &&
          readFileSync(path.join(repoRoot, assetPath), "utf8").includes(font),
      )
    ) {
      fail(`asset entry ${entry.id} declares unused SVG font family: ${font}`);
    }
  }
}

function validateLocators(relative, locators, occurrences, label) {
  if (!regularFileSet.has(relative))
    fail(`${label} path is not a source regular file: ${relative}`);
  if (
    !Array.isArray(locators) ||
    locators.length !== occurrences ||
    new Set(locators).size !== locators.length ||
    locators.some((locator) => !/^[1-9][0-9]*:[1-9][0-9]*$/.test(locator))
  ) {
    fail(
      `${label} locators must be unique line:column coordinates for every occurrence`,
    );
  }
}

const actualEmbedded = discoveredEmbedded;
const expectedEmbedded = new Map();
const usedRightsProfiles = new Set();
for (const entry of manifest.embedded_data ?? []) {
  rejectUnknownKeys(
    entry,
    new Set([
      "id",
      "path",
      "locators",
      "media_type",
      "encoding",
      "decoded_sha256",
      "decoded_bytes",
      "occurrences",
      "rights_profile",
    ]),
    `embedded entry ${entry.id ?? "<missing-id>"}`,
  );
  const profile = manifest.rights_profiles?.[entry.rights_profile];
  if (!profile) {
    fail(
      `embedded entry ${entry.id ?? "<missing-id>"} has an unknown rights_profile`,
    );
  } else {
    usedRightsProfiles.add(entry.rights_profile);
    rejectUnknownKeys(
      profile,
      new Set([
        "classification",
        "license",
        "usage",
        "copyright_holder",
        "status",
        "trademark_status",
        "provenance",
      ]),
      `rights profile ${entry.rights_profile}`,
    );
    validateRightsRecord(
      { ...profile, id: entry.id },
      `embedded entry ${entry.id ?? "<missing-id>"}`,
    );
  }
  validateLocators(
    entry.path,
    entry.locators,
    entry.occurrences,
    `embedded entry ${entry.id}`,
  );
  if (
    !/^(?:base64|percent|raw)$/.test(entry.encoding ?? "") ||
    !shaPattern.test(entry.decoded_sha256 ?? "") ||
    !Number.isSafeInteger(entry.decoded_bytes) ||
    entry.decoded_bytes <= 0 ||
    !Number.isSafeInteger(entry.occurrences) ||
    entry.occurrences <= 0
  ) {
    fail(`embedded entry ${entry.id} has invalid payload coordinates`);
  }
  const signature = [
    entry.path,
    entry.media_type,
    entry.encoding,
    entry.decoded_sha256,
    entry.decoded_bytes,
  ].join("\0");
  if (expectedEmbedded.has(signature))
    fail(`duplicate embedded-data signature: ${entry.id}`);
  expectedEmbedded.set(signature, entry);
}
for (const profileName of Object.keys(manifest.rights_profiles ?? {})) {
  if (!idPattern.test(profileName))
    fail(`invalid rights profile id: ${profileName}`);
  if (!usedRightsProfiles.has(profileName))
    fail(`unused rights profile: ${profileName}`);
}

for (const actual of actualEmbedded.decoded) {
  const signature = [
    actual.path,
    actual.media_type,
    actual.encoding,
    actual.decoded_sha256,
    actual.decoded_bytes,
  ].join("\0");
  const expected = expectedEmbedded.get(signature);
  if (!expected) {
    fail(
      `unlisted embedded ${actual.encoding} ${actual.media_type}: ${actual.path} sha256=${actual.decoded_sha256} bytes=${actual.decoded_bytes} occurrences=${actual.occurrences} locators=${actual.locators.join(",")}`,
    );
  } else if (expected.occurrences !== actual.occurrences) {
    fail(
      `embedded entry ${expected.id} occurrence count is ${actual.occurrences}, expected ${expected.occurrences}`,
    );
  } else if (
    JSON.stringify(expected.locators) !== JSON.stringify(actual.locators)
  ) {
    fail(`embedded entry ${expected.id} source locators are stale`);
  }
  expectedEmbedded.delete(signature);
}
for (const entry of expectedEmbedded.values())
  fail(`embedded entry no longer exists: ${entry.id}`);

const expectedMarkers = new Map();
const usedMarkerProfiles = new Set();
for (const entry of manifest.data_uri_markers ?? []) {
  rejectUnknownKeys(
    entry,
    new Set([
      "id",
      "path",
      "locators",
      "statement_sha256",
      "statement_bytes",
      "occurrences",
      "purpose",
      "marker_kind",
      "rights_profile",
    ]),
    `data URI marker ${entry.id ?? "<missing-id>"}`,
  );
  if (!idPattern.test(entry?.id ?? ""))
    fail(`data URI marker ${entry?.id ?? "<missing-id>"} has an invalid id`);
  else if (seenIds.has(entry.id))
    fail(`duplicate source asset id: ${entry.id}`);
  else seenIds.add(entry.id);
  if (!allowedMarkerKinds.has(entry.marker_kind))
    fail(`data URI marker ${entry.id} has an invalid marker_kind`);
  const markerProfile = manifest.marker_profiles?.[entry.rights_profile];
  if (!markerProfile) {
    fail(`data URI marker ${entry.id} has an unknown rights_profile`);
  } else {
    usedMarkerProfiles.add(entry.rights_profile);
    rejectUnknownKeys(
      markerProfile,
      new Set([
        "license",
        "usage",
        "copyright_holder",
        "status",
        "license_evidence",
      ]),
      `marker profile ${entry.rights_profile}`,
    );
    if (
      markerProfile.license !== "Apache-2.0" ||
      markerProfile.status !== "cleared"
    ) {
      fail(
        `marker profile ${entry.rights_profile} must be cleared Apache-2.0 source text`,
      );
    }
    if (
      !Array.isArray(markerProfile.usage) ||
      markerProfile.usage.length === 0 ||
      markerProfile.usage.some((usage) => !allowedUsage.has(usage))
    ) {
      fail(`marker profile ${entry.rights_profile} has invalid usage`);
    }
    if (
      typeof markerProfile.copyright_holder !== "string" ||
      markerProfile.copyright_holder.trim() === ""
    ) {
      fail(`marker profile ${entry.rights_profile} lacks copyright_holder`);
    }
    if (
      !Array.isArray(markerProfile.license_evidence) ||
      markerProfile.license_evidence.length === 0
    ) {
      fail(`marker profile ${entry.rights_profile} lacks license evidence`);
    } else {
      markerProfile.license_evidence.forEach((item, index) => {
        verifyFileEvidence(
          item,
          `marker profile ${entry.rights_profile}.license_evidence[${index}]`,
        );
      });
    }
  }
  validateLocators(
    entry.path,
    entry.locators,
    entry.occurrences,
    `data URI marker ${entry.id}`,
  );
  if (
    !shaPattern.test(entry.statement_sha256 ?? "") ||
    !Number.isSafeInteger(entry.statement_bytes) ||
    entry.statement_bytes <= 0 ||
    !Number.isSafeInteger(entry.occurrences) ||
    entry.occurrences <= 0 ||
    typeof entry.purpose !== "string" ||
    entry.purpose.trim() === ""
  ) {
    fail(
      `data URI marker ${entry.id} has invalid explicit-fixture coordinates`,
    );
  }
  const signature = [
    entry.path,
    entry.statement_sha256,
    entry.statement_bytes,
  ].join("\0");
  if (expectedMarkers.has(signature))
    fail(`duplicate data URI marker signature: ${entry.id}`);
  expectedMarkers.set(signature, entry);
}
for (const profileName of Object.keys(manifest.marker_profiles ?? {})) {
  if (!idPattern.test(profileName))
    fail(`invalid marker profile id: ${profileName}`);
  if (!usedMarkerProfiles.has(profileName))
    fail(`unused marker profile: ${profileName}`);
}
for (const actual of actualEmbedded.markers) {
  const signature = [
    actual.path,
    actual.statement_sha256,
    actual.statement_bytes,
  ].join("\0");
  const expected = expectedMarkers.get(signature);
  if (!expected) {
    fail(
      `unclassified media data URI marker: ${actual.path} sha256=${actual.statement_sha256} bytes=${actual.statement_bytes} occurrences=${actual.occurrences} locators=${actual.locators.join(",")}`,
    );
  } else if (expected.occurrences !== actual.occurrences) {
    fail(
      `data URI marker ${expected.id} occurrence count is ${actual.occurrences}, expected ${expected.occurrences}`,
    );
  } else if (
    JSON.stringify(expected.locators) !== JSON.stringify(actual.locators)
  ) {
    fail(`data URI marker ${expected.id} source locators are stale`);
  }
  expectedMarkers.delete(signature);
}
for (const entry of expectedMarkers.values())
  fail(`data URI marker no longer exists: ${entry.id}`);

if (failures.length > 0) {
  console.error("Carryforth source asset inventory check failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(
  `Carryforth source asset inventory passed (${candidates.length} files, ${actualEmbedded.decoded.length} embedded payloads, ${actualEmbedded.markers.length} explicit URI markers; no bare-base64 or complete numeric-array media).`,
);
