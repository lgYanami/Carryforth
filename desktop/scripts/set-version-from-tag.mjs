import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const version = process.argv[2];

if (!version) {
  console.error("Usage: node scripts/set-version-from-tag.mjs <version>");
  process.exit(1);
}

if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(
    `Invalid version "${version}". Expected semver format (e.g. 1.2.3 or 1.2.3-beta.1)`,
  );
  process.exit(1);
}

const packageJsonPath = resolve(process.cwd(), "package.json");
const tauriConfigPath = resolve(process.cwd(), "src-tauri/tauri.conf.json");
const cargoTomlPath = resolve(process.cwd(), "src-tauri/Cargo.toml");
const cargoLockPath = resolve(process.cwd(), "src-tauri/Cargo.lock");

const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8"));
packageJson.version = version;
writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`);
console.log(`Set package.json to ${version}`);

const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
tauriConfig.version = version;
writeFileSync(tauriConfigPath, `${JSON.stringify(tauriConfig, null, 2)}\n`);
console.log(`Set tauri.conf.json to ${version}`);

const cargoToml = readFileSync(cargoTomlPath, "utf8");
const updatedCargoToml = cargoToml.replace(
  /^version = ".*"$/m,
  `version = "${version}"`,
);
writeFileSync(cargoTomlPath, updatedCargoToml);
console.log(`Set Cargo.toml to ${version}`);

// Keep the root package entry in the checked-in lockfile synchronized without
// invoking `cargo update`. A release build must never re-resolve dependencies;
// only this workspace package's own version is allowed to change.
const cargoLock = readFileSync(cargoLockPath, "utf8");
const packageEntry =
  /(\[\[package\]\]\nname = "buzz-desktop"\nversion = ")[^"]+("\n)/g;
const matches = [...cargoLock.matchAll(packageEntry)];
if (matches.length !== 1) {
  console.error(
    `Expected exactly one buzz-desktop package entry in Cargo.lock, found ${matches.length}`,
  );
  process.exit(1);
}
const updatedCargoLock = cargoLock.replace(packageEntry, `$1${version}$2`);
writeFileSync(cargoLockPath, updatedCargoLock);
console.log(`Set Cargo.lock local package entry to ${version}`);
