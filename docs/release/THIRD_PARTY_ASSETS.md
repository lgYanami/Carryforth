# Packaged Asset Provenance and License Inventory

Status: release audit in progress  
Inventory data: [`packaged-assets.json`](packaged-assets.json)  
Gate: [`scripts/check-release-asset-inventory.sh`](../../scripts/check-release-asset-inventory.sh)

This document covers the non-code media, fonts, project binaries, and material
container inputs on the first public Carryforth release surface:

- unsigned Linux Carryforth Desktop (`.deb` and AppImage);
- Desktop-bundled ACP/agent/developer sidecars;
- standalone `cf`;
- the Carryforth Local Relay OCI image.

It is an engineering provenance record, not legal advice. A file being present
in the Apache-2.0 upstream repository is useful history, but it does not by
itself establish that an embedded third-party work or trademark can be reused
in a new product distribution. Entries without source and redistribution
evidence remain release blockers.

## Inventory method

The audit used four evidence classes:

1. the current tracked file and its SHA-256 content;
2. the commit and upstream pull request that introduced it;
3. an asset-local license or a reproducible in-repository generator;
4. the actual Tauri `externalBin`, Vite/public asset, release workflow, and
   Dockerfile packaging paths.

`tree_sha256` in the JSON inventory is the SHA-256 of the sorted `sha256sum`
lines for every tracked file matched by that entry. It binds both paths and
bytes. The gate fails if a matched set changes without an explicit inventory
review.

Generated `desktop/dist` files are not independent sources. Vite copies or
transforms the tracked files recorded here, including Inter WOFF2 subsets. A
clean release build must regenerate `dist`; it must not treat a stale local
`dist` directory as provenance evidence.

## Cleared packaged artwork

| Asset | Packaged source | Evidence | License/status |
| --- | --- | --- | --- |
| Carryforth app glyph and wordmark | `desktop/public/carryforth.svg`, `desktop/public/landing/carryforth-wordmark.svg` | Created in Carryforth commit `8d02a4750e5cf0ac690ffe712341217f1994b063` | Apache-2.0; cleared |
| Carryforth application icons | `desktop/src-tauri/icons/**` | Preferred source is `carryforth-source.svg`; platform renditions were generated in the same Carryforth commit | Apache-2.0; cleared |
| Card texture | `desktop/src/shared/ui/assets/card-texture.png` | Reproducible procedural SVG recipe in `desktop/scripts/texture-card/generate-card-texture.mjs`; generator reads no external image | Apache-2.0; cleared |
| Poof animation and sound | `desktop/public/pow/**` | Exact upstream files from [EmergeTools/Pow at `1b4b1dda`](https://github.com/EmergeTools/Pow/tree/1b4b1dda28c50b95f0872927ee2226fe8b58950e); all six media hashes were verified byte-for-byte. The directory ships its copyright and MIT license. | MIT; cleared |

The Carryforth wordmark refers to Inter by font-family name but does not embed
font bytes. Inter's separately bundled WOFF2 files are covered below.

## Unresolved Desktop asset blockers

| Blocker | Files | Known provenance | Missing evidence / required resolution |
| --- | --- | --- | --- |
| Runtime-provider marks | `desktop/public/runtime-icons/**` | Added by [block/buzz#2026](https://github.com/block/buzz/pull/2026), merge `2b0f5e9f…`. The Goose vector contains paths traceable to the official `aaif-goose/goose` artwork. | No recorded source or redistribution/trademark terms for the Claude or Codex PNGs; no bundled asset notice or approved downstream mark-use record for Goose. Replace all with Carryforth-owned generic runtime glyphs/text, or record authoritative license and trademark permission. |
| Onboarding-provider marks | `desktop/src/features/onboarding/assets/harness-logos/**` | Added by [block/buzz#2039](https://github.com/block/buzz/pull/2039), merge `66a0f7bc…`. | No recorded source, copyright license, or trademark terms for ChatGPT, Claude, and Goose raster artwork. Replace or obtain and document permission. |
| Starter-team character art | `desktop/public/onboarding/starter-team/**` | Added by [block/buzz#2032](https://github.com/block/buzz/pull/2032), merge `1aab50e9…`; the PR says APNGs were assembled from “Wes's” Fizz, Honey, and Bumble frame sequences. | Source frames, explicit ownership/redistribution attestation, and downstream character/trademark permission are absent. Replace with Carryforth-owned art or obtain a written provenance and license record. |
| Notification audio | `desktop/public/sounds/*.mp3` | Added by [block/buzz#968](https://github.com/block/buzz/pull/968), merge `4e4dc723…`. | No source, creator, or license is recorded for the twelve audio files. Replace with newly commissioned/generated assets with source and license, or obtain proof of rights. |
| Waveform thumbnails | `desktop/public/sounds/*.svg` | The PR generator derives them from the corresponding MP3 PCM data. | They inherit the unresolved provenance of the source audio; resolve or regenerate after replacing the audio. |
| Inter Variable font | `@fontsource-variable/inter@5.2.8` emitted by Vite | Package license: Copyright 2016 The Inter Project Authors, SIL Open Font License 1.1. | OFL requires the copyright and license to accompany redistributed font software. Stage the OFL text in the installed Desktop/release notices and verify it exists inside both `.deb` and AppImage. |

These entries are blockers for publishing the current Desktop artifact. A
release job must not merely suppress the gate. Removing an asset is acceptable
only when its code references and generated output are removed together;
replacing it requires a new inventory record and hash.

## Packaged programs and sidecars

Tauri's current `externalBin` list and `scripts/bundle-sidecars.sh` agree on the
following five source-built programs:

| Packaged name | Cargo package | Destinations | Project source license |
| --- | --- | --- | --- |
| `binaries/buzz-acp` | `buzz-acp` | Desktop sidecar | Apache-2.0 |
| `binaries/buzz-agent` | `buzz-agent` | Desktop sidecar | Apache-2.0 |
| `binaries/buzz-dev-mcp` | `buzz-dev-mcp` | Desktop sidecar | Apache-2.0 |
| `binaries/git-credential-nostr` | `git-credential-nostr` | Desktop sidecar | Apache-2.0 |
| `binaries/cf` | `carryforth-cli` | Desktop sidecar and standalone archive | Apache-2.0 |

The Desktop Rust application is Apache-2.0 project source. The Local Relay
image additionally packages source-built `buzz-relay`, `buzz-admin`, and
`buzz-pair-relay`, also declared Apache-2.0 through workspace package metadata.

This clears the provenance of Carryforth's own program source, not the whole
linked binary. Before release, generate a tag-bound third-party license report
and SPDX or CycloneDX SBOM for:

- each Rust program's resolved Cargo dependency closure;
- the Desktop's pnpm/Vite dependency closure;
- native libraries embedded by AppImage and installed dependencies declared by
  the Debian package;
- the Local Relay runtime image's Debian packages.

The release manifest must name the Git commit, target, binary hash, SBOM hash,
license-report hash, and the archived result of a locked RustSec audit for the
same dependency closure. Until those artifacts are generated and shipped, the
binary dependency obligation remains blocked in the JSON inventory.

## Relay container provenance blocker

The current Dockerfile resolves `rust:1.95-bookworm` and
`debian:bookworm-slim` by mutable tag and installs unversioned apt packages.
The Rust image is build-only, while the Debian runtime image and its installed
packages are distributed in the final OCI layers. Both still need provenance.

For a release tag:

1. pin each base image by digest;
2. record the resolved digest and build tool versions in provenance;
3. use a reproducible package snapshot or record exact installed Debian package
   versions in the final image SBOM;
4. attach the image digest, SBOM, and provenance to the same release commit.

This is independent of the media blockers and must be closed before the Relay
image is called reproducible.

## Desktop bundle identity and data-migration blocker

The current Tauri identifier remains `xyz.block.buzz.app`. That temporary
technical coordinate protects existing keyring and app-data continuity, but it
cannot be shipped as the independent Carryforth product identity. Changing it
without migration would make existing local identity and business state appear
lost; continuing to package it would publish under an upstream reverse-DNS
identity and let local-only cleanup run inside the legacy store.

Before any Desktop package is uploaded, choose a Carryforth-owned identifier
and prove a fail-closed copy/readback migration for identity, Community,
messages, Agents, Project View, Documents, Project Context, and Meetings. The
first migrated release must retain the legacy store as a recovery copy and
must stop on conflicting data rather than overwrite either side.

## Operational and governance release blockers

Four additional release obligations were already part of the written release
contract but were not previously represented in the machine-readable gate.
They are now explicit blockers:

| Obligation | Evidence required before clearance |
| --- | --- |
| Owner-signed Project capability bootstrap | From the published local bootstrap, initialize an Owner identity, idempotently enable Project View v3, Documents, Project Context Edge, and Meeting, then canonically read every advertised capability back. A hand-modified development database is not evidence. |
| Existing-data upgrade and readback | Freeze an immutable supported baseline; migrate only a recoverable copy; compare identity, Community, messages, managed Agents, schema/migrations, Project revisions, Documents, Context, and Meeting history before and after; preserve the source copy. This is separate from choosing and migrating the Desktop bundle coordinate. |
| Clean-room published-artifact E2E | On a machine without a repository checkout, old app data, Block access, or private credentials, use only published artifacts to create an Owner Community and exercise messages, a managed Agent, all three Project domains, and a naturally completed 4–6-round Meeting. Restart and read back state, then repeat with legacy service domains blocked. |
| Human private-reporting and release governance | Human maintainers must publish stable private security and conduct-reporting routes and record accountable maintainers, official artifact namespace/retention, supported platforms, signing custodians and rotation/loss policy, and the immutable upgrade baseline. An implementation agent must not invent these values. |

None of these four obligations currently has accepted tag-bound evidence, so
all remain `blocked`.

## Evidence contract for release obligations

Changing a `release_obligations` entry to `cleared` is not sufficient. The
same manifest entry must include an `evidence` object containing:

- schema `carryforth.release-obligation-evidence/v1`;
- an exact protected `v<semver>` release tag;
- the path `docs/release/evidence/<tag>/<obligation-id>.json`;
- the lowercase SHA-256 of that tracked evidence file.

The evidence JSON must repeat the schema, release tag, obligation ID and
40-character source commit, declare `result: passed`, record an RFC 3339 UTC
timestamp, and contain at least one named passed check with a non-empty evidence
description. The tracked evidence path must be a regular file, not a symlink.
In strict release mode the tag must be available, both the tag
and the current release `HEAD` must resolve to the recorded source commit, and
the file hash must match the manifest. Consequently a status-only edit, an
untracked report, evidence from another tag, or stale evidence bytes all fail
closed.

## Gate behavior

Run the inventory integrity audit during development:

```bash
./scripts/check-release-asset-inventory.sh
```

This checks only the explicit asset roots in `packaged-assets.json`, validates
their path/byte-set hashes, validates the Tauri sidecar set against Cargo
package licenses, and reports unresolved entries. It intentionally does not
claim that unrelated repository fixtures are packaged assets.

The release form is stricter:

```bash
./scripts/check-release-asset-inventory.sh --release
```

It fails while any inventory entry or release obligation is `blocked`. There
are currently **12** blockers: five packaged asset/font entries and seven
release obligations. That is the expected result until every asset blocker has
been replaced or licensed and every obligation has tag-bound evidence that
passes the contract above.

## Closure checklist

- [ ] Replace or clear both Provider-logo groups.
- [ ] Replace or clear Fizz, Honey, and Bumble art.
- [ ] Replace or clear all notification MP3s and regenerate their waveforms.
- [ ] Bundle Inter's OFL copyright/license with `.deb` and AppImage.
- [ ] Generate and publish license reports and SBOMs for Desktop, all sidecars,
      standalone `cf`, and the Relay image.
- [ ] Pin Relay build/runtime images and close Debian package provenance.
- [ ] Choose a Carryforth bundle identifier and complete the no-loss app-data
      and keyring migration.
- [ ] Prove the published Owner-signed Project capability bootstrap and
      canonical capability readback.
- [ ] Freeze an upgrade baseline and prove existing-data migration/readback on
      a recoverable copy.
- [ ] Complete the published-artifact-only clean-room E2E and blocked-legacy-
      domain run.
- [ ] Have Human maintainers publish the private security/conduct routes and
      record the remaining release-governance decisions.
- [ ] Re-run the gate on the immutable release tag and archive its output with
      release provenance.
