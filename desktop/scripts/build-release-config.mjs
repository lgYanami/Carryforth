import { writeFileSync } from "node:fs";
import { resolve } from "node:path";

// Write a tauri.release.conf.json with release-only overrides.
//
// Tauri's --config flag merges the provided JSON on top of the base
// tauri.conf.json, so this file must contain ONLY the delta fields —
// not a copy of the base config.
//
// Public Carryforth community builds deliberately do not produce updater
// artifacts and do not embed a release endpoint. Platform signing, if added in
// a future release lane, must remain orthogonal to this product configuration.

const outputConfigPath = resolve(
  process.cwd(),
  process.argv[2] ?? "src-tauri/tauri.release.conf.json",
);

const releaseConfig = {
  bundle: {
    macOS: {
      minimumSystemVersion: "10.15",
    },
    createUpdaterArtifacts: false,
  },
};

writeFileSync(outputConfigPath, `${JSON.stringify(releaseConfig, null, 2)}\n`);
console.log(`Wrote non-updating release config to ${outputConfigPath}`);
