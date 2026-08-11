# Releasing Carryforth

## Current status

Carryforth has not yet declared an independent stable binary release. A public
community-release lane now builds unsigned Linux artifacts without Block
organization access or secrets, but those artifacts remain release candidates
until clean-room, migration, SBOM, and packaged-asset acceptance is complete.

The current public lanes are:

- a protected `v<semver>` tag (or `workflow_dispatch` retry on that exact tag)
  invokes `.github/workflows/release.yml` and builds Linux x86_64 Carryforth
  Desktop `.deb`/`.AppImage` packages, a `cf` tarball, `SHA256SUMS`, and GitHub
  build provenance. Desktop packages are explicitly marked community-unsigned;
- a protected `relay-v<semver>` tag invokes `.github/workflows/docker.yml` and
  publishes amd64/arm64 `carryforth-relay` images under the invoking GitHub
  owner's lowercase GHCR namespace. GHCR does not inherit repository
  visibility automatically; the workflow logs out and proves anonymous digest
  access, failing until the package owner explicitly makes the package public;
- macOS has only a manually dispatched unsigned source canary. macOS and
  Windows are not formal release platforms.

There is no rolling updater release, hosted-service configuration injection,
or Block signing step. The community lane is not evidence of a stable release
until the remaining release-readiness gates pass.

The packaged-asset gate is deliberately fail-closed. Its current blockers and
provenance evidence are recorded in
[`docs/release/THIRD_PARTY_ASSETS.md`](docs/release/THIRD_PARTY_ASSETS.md); do
not publish by removing or weakening that gate. The inventory currently
reports 12 blockers: five packaged asset/font entries and seven release
obligations.

The implementation and acceptance contract is maintained in
[`docs/lora/stage/carryforth/open-source-release-surface-plan.md`](docs/lora/stage/carryforth/open-source-release-surface-plan.md).

## First public release surface

The first release is expected to contain independently versioned artifacts for:

| Component | Version authority | Community artifact |
| --- | --- | --- |
| Desktop | `desktop/package.json` and synchronized Tauri manifests | `carryforth-desktop-<version>-<platform>` |
| Local Relay | `crates/buzz-relay/Cargo.toml` | `carryforth-relay:<version>` OCI image |
| Agent CLI | workspace / `crates/carryforth-cli/Cargo.toml` | `cf-<version>-<target>` |
| Local deployment | release manifest | version-pinned bootstrap/compose bundle (not yet stable) |

The first formal support matrix is Linux-focused. Web, Mobile, benchmarks, and
other retained source trees are not automatically release artifacts. The
inherited Helm/Kubernetes and hosted Push Gateway executables are retired and
are not Carryforth release targets.

Internal crate and binary names may still use `buzz-*` for compatibility. A
public artifact may package those binaries, but the artifact name, user-facing
metadata, release URL, and container namespace must use Carryforth.

## Release invariants

Every Carryforth release must satisfy all of the following:

1. **Clean source** — build from an immutable tag whose index and worktree are
   clean. Untracked files, missing migrations, or undeclared generated inputs
   make the release fail closed.
2. **Public dependencies** — the build works without Block VPN, internal DNS,
   internal registries, private Buildkite pipelines, or Block signing actions.
3. **No hidden remote fallback** — Desktop and Relay do not reconnect to a
   legacy hosted community, account, updater, or push endpoint.
4. **Data continuity** — migrations are forward-only and tested only against
   explicit scratch databases. Release scripts never reset a user's local
   database, keyring, app-data, Agent state, or Docker volumes.
5. **Reproducible identity** — artifact version, Git commit, toolchains,
   checksums, SBOM, and provenance identify the same source.
6. **Honest signing** — unsigned community builds are labeled as unsigned.
   Signing is added only through Carryforth-owned credentials and does not
   change application behavior.
7. **Attribution** — `LICENSE`, `NOTICE`, and `UPSTREAM.md` ship with source
   and binary distributions as required.

## Required release artifacts

A stable release is incomplete without:

- Desktop package(s) for every platform claimed as supported;
- a multi-architecture Local Relay OCI image;
- `cf` binaries for every claimed target;
- a version-pinned local deployment/bootstrap bundle;
- SHA-256 checksums;
- an SPDX or CycloneDX SBOM;
- build provenance tied to the immutable tag;
- release notes, known limitations, and data-migration instructions.

## Release-obligation evidence

Operational acceptance and Human governance decisions are release inputs, not
informal follow-up tasks. In particular, publication remains blocked until the
machine-readable inventory records passed evidence for:

- Owner-signed Project View v3, Document, Project Context Edge, and Meeting
  capability bootstrap plus canonical readback from the published local stack;
- existing-data migration/readback from a Human-selected immutable baseline,
  performed on a recoverable copy;
- a published-artifact-only clean-room end-to-end run, including restart,
  canonical state readback, and a run with legacy service domains blocked;
- stable private security/conduct reporting routes and the remaining Human
  release-governance decisions.

Every cleared release obligation must point to a tracked
`docs/release/evidence/<v-semver-tag>/<obligation-id>.json` record. The
inventory binds that record by schema and SHA-256; strict mode also verifies
the recorded source commit against the release tag and `HEAD`. Merely changing
`release_status` to `cleared`, attaching an untracked report, or reusing a
report from another tag does not satisfy the gate.

## Release candidate procedure

The public workflows enforce source/tag and artifact boundaries, but the
following procedure remains the acceptance contract. A successful build alone
does not authorize calling a release stable.

1. Freeze the supported component/platform matrix and release version.
2. Verify the candidate tag contains the complete migration chain and no
   untracked or local-only input.
3. Build all artifacts on public clean-room runners.
4. Start a fresh local stack from only the published artifacts.
5. Create a local identity and Owner community; verify messages, managed
   Agents, Project View, Documents, Project Context, and a naturally completed
   4–6-round Meeting.
6. Stop, restart, and upgrade the stack; read back the same identity and
   canonical data.
7. Repeat with legacy Buzz/Block service domains blocked and confirm there is
   no fallback or push/updater request.
8. Run the supported existing-data migration on a recoverable data copy and
   compare pubkey, Community, messages, Agent state, project revisions, and
   Meeting history before and after.
9. Scan unpacked artifacts for retired product names, remote endpoints,
   secrets, internal URLs, and assets without redistribution evidence.
10. Publish the candidate, checksums, SBOM, provenance, and acceptance record.

The acceptance record must close each machine-readable release obligation with
tag-bound evidence as described above. Until then the candidate may be useful
for development testing, but it is not an authorized stable release.

No individual platform result substitutes for another. A platform enters the
support matrix only after its signing, application identity, keyring/app-data
migration, and clean-machine checks pass.

## Maintainer decisions still required

The following values must be selected and recorded by Human maintainers; an
implementation agent must not invent them:

- the permanent reverse-DNS Desktop identifier and its migration owner;
- the official GHCR namespace and artifact retention policy;
- supported release platforms beyond the initial Linux baseline;
- signing identities, secret custodians, and key rotation/loss procedures;
- the stable security and private conduct-reporting contacts;
- the exact immutable baseline from which existing-data upgrade support is
  promised.

Until those decisions and the public workflows are in place, do not advertise
the inherited `just release-*` recipes or upstream workflows as Carryforth's
official release process.
