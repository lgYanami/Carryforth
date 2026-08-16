#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";

function fail(message) {
  process.stderr.write(`[local-env] ERROR: ${message}\n`);
  process.exit(1);
}

function parseArguments(argv) {
  let envFile;
  let templateFile;

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--target") {
      envFile = argv[index + 1];
      index += 1;
    } else if (argument === "--source-template") {
      templateFile = argv[index + 1];
      index += 1;
    } else {
      fail(`unknown argument: ${argument}`);
    }
  }

  if (!envFile || !templateFile) {
    fail(
      "usage: update-local-env.mjs --target <path> --source-template <path>",
    );
  }
  return {
    envFile: path.resolve(envFile),
    templateFile: path.resolve(templateFile),
  };
}

function assertRegularOrMissing(filePath, label) {
  let metadata;
  try {
    metadata = fs.lstatSync(filePath);
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    fail(`${label} must be a regular file: ${filePath}`);
  }
  return true;
}

function quoteValue(value, name) {
  if (value.includes("\0") || value.includes("\n") || value.includes("\r")) {
    fail(`${name} cannot contain NUL or newline characters`);
  }
  const escaped = value
    .replaceAll("\\", "\\\\")
    .replaceAll('"', '\\"')
    .replaceAll("$", "\\$")
    .replaceAll("`", "\\`");
  return `"${escaped}"`;
}

function requestedUpdates() {
  const mapping = [
    ["CARRYFORTH_LOCAL_WORKER_ENABLED", "BUZZ_SEMANTIC_WORKER_ENABLED"],
    [
      "CARRYFORTH_LOCAL_QUERY_HTTP_AVAILABLE",
      "BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE",
    ],
    [
      "CARRYFORTH_LOCAL_COORDINATE_SEARCH_HTTP_AVAILABLE",
      "CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE",
    ],
    [
      "CARRYFORTH_LOCAL_ONE_HOP_SEARCH_HTTP_AVAILABLE",
      "CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE",
    ],
    ["CARRYFORTH_LOCAL_SEMANTIC_API_KEY", "BUZZ_SEMANTIC_API_KEY"],
    ["CARRYFORTH_LOCAL_SEMANTIC_BASE_URL", "BUZZ_SEMANTIC_BASE_URL"],
    ["CARRYFORTH_LOCAL_SEMANTIC_REQUEST_MODEL", "BUZZ_SEMANTIC_REQUEST_MODEL"],
    ["CARRYFORTH_LOCAL_LLM_API_KEY", "LLM_API_KEY"],
    ["CARRYFORTH_LOCAL_LLM_BASE_URL", "LLM_BASE_URL"],
    ["CARRYFORTH_LOCAL_LLM_MODEL", "LLM_MODEL"],
    ["CARRYFORTH_LOCAL_FLEET_POLICY", "BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY"],
    ["CARRYFORTH_LOCAL_RELAY_PRIVATE_KEY", "BUZZ_RELAY_PRIVATE_KEY"],
  ];
  const updates = new Map();
  for (const [source, target] of mapping) {
    if (Object.hasOwn(process.env, source)) {
      updates.set(target, process.env[source]);
    }
  }
  return updates;
}

function applyUpdates(contents, updates, migrateLegacyBind) {
  const lines = contents.split("\n");
  const written = new Set();
  const output = [];
  for (const line of lines) {
    const assignment = line.match(
      /^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=/,
    );
    const name = assignment?.[1];
    if (
      migrateLegacyBind &&
      name === "BUZZ_BIND_ADDR" &&
      /^\s*(?:export\s+)?BUZZ_BIND_ADDR\s*=\s*(?:["']?)0\.0\.0\.0:3000(?:["']?)\s*$/.test(
        line,
      )
    ) {
      output.push(
        "# Carryforth migrated the former source-development wildcard default to loopback.",
        "BUZZ_BIND_ADDR=127.0.0.1:3000",
      );
      continue;
    }
    if (!name || !updates.has(name)) {
      output.push(line);
      continue;
    }
    if (!written.has(name)) {
      output.push(`${name}=${quoteValue(updates.get(name), name)}`);
      written.add(name);
    }
  }

  const missing = [...updates.keys()].filter((name) => !written.has(name));
  if (missing.length > 0) {
    while (output.length > 0 && output.at(-1) === "") output.pop();
    output.push(
      "",
      "# Local source-development semantic defaults (managed by source launchers).",
    );
    for (const name of missing) {
      output.push(`${name}=${quoteValue(updates.get(name), name)}`);
    }
    output.push("");
  }

  return output.join("\n");
}

function writeAtomically(filePath, contents) {
  const directory = path.dirname(filePath);
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const temporary = path.join(
    directory,
    `.${path.basename(filePath)}.tmp-${process.pid}-${Date.now()}`,
  );
  let descriptor;
  let operationError;
  try {
    descriptor = fs.openSync(temporary, "wx", 0o600);
    fs.writeFileSync(descriptor, contents, { encoding: "utf8" });
    fs.fsyncSync(descriptor);
    fs.closeSync(descriptor);
    descriptor = undefined;
    fs.renameSync(temporary, filePath);
    fs.chmodSync(filePath, 0o600);
  } catch (error) {
    operationError = error;
  }

  if (descriptor !== undefined) {
    try {
      fs.closeSync(descriptor);
    } catch (error) {
      operationError ??= error;
    }
  }

  let cleanupError;
  try {
    fs.unlinkSync(temporary);
  } catch (error) {
    if (error?.code !== "ENOENT") cleanupError = error;
  }
  if (operationError) throw operationError;
  if (cleanupError) throw cleanupError;
}

const { envFile, templateFile } = parseArguments(process.argv.slice(2));
const envExists = assertRegularOrMissing(envFile, "environment file");
if (!assertRegularOrMissing(templateFile, "environment template")) {
  fail(`environment template is missing: ${templateFile}`);
}

const original = fs.readFileSync(envExists ? envFile : templateFile, "utf8");
const updated = applyUpdates(original, requestedUpdates(), envExists);
if (!envExists || updated !== original) {
  writeAtomically(envFile, updated);
} else {
  fs.chmodSync(envFile, 0o600);
}
