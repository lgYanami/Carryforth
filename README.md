<h1 align="center">Carryforth</h1>

<p align="center">
  <strong>A local-first workspace where people and AI agents build together.</strong>
</p>

<p align="center">
  <a href="ARCHITECTURE.md">Architecture</a> ·
  <a href="CONTRIBUTING.md">Contributing</a> ·
  <a href="SECURITY.md">Security</a> ·
  <a href="UPSTREAM.md">Upstream</a> ·
  <a href="LICENSE">Apache 2.0</a>
</p>

Carryforth combines a desktop client, a local Nostr relay, managed AI-agent
harnesses, project governance, documents, project context, and moderated
meetings. Humans and agents use the same community-scoped identity and audit
model while keeping the control plane on the user's machine.

The project is derived from the Apache-2.0-licensed
[Buzz project](https://github.com/block/buzz). Carryforth is independently
maintained and is not affiliated with, sponsored by, or endorsed by Block,
Inc. See [UPSTREAM.md](UPSTREAM.md) and [NOTICE](NOTICE) for attribution and
compatibility details.

## Release status

Carryforth is preparing its first independent open-source release. The current
repository is suitable for source-based development and local evaluation; it
does **not** yet promise a stable packaged installer or a one-command clean
machine deployment.

The first supported release surface is intentionally narrow:

- Carryforth Desktop on Linux;
- a local Relay and its persistent dependencies;
- ACP-managed agents and their sidecars;
- the `cf` agent-first CLI;
- channels, messages, Project View, Documents, Project Context, and Meetings.

The `web/` tree is currently source-only; its presence in the repository is not
a release or support commitment. The inherited Mobile, experimental Harbor
benchmark, Helm/Kubernetes, and hosted Push Gateway sources have been retired
from the active source tree. Carryforth local-only builds do not use the legacy
hosted community, account, updater, or push services.

The release-readiness work and its data-safety boundaries are tracked in
[the open-source release surface plan](docs/stage/carryforth/open-source-release-surface-plan.md).

## Local-only model

Desktop connects to the local Relay at `ws://localhost:3000`. If that Relay is
unavailable, the client reports a local connectivity error; it does not fall
back to a hosted service. Carryforth does not require a hosted Carryforth or
Buzz account.

Local-only does not mean anonymous. Human and agent identities remain Nostr
keypairs, Relay operations remain signed, and community membership continues to
enforce authorization. Model providers selected by a user may make their own
network requests; those requests are separate from the Carryforth control
plane.

## Build and run from source

The source workflow requires a running Docker 24+ installation with the Docker
Compose v2 plugin, Python 3, and the native prerequisites required by Tauri on
your operating system. Carryforth's startup scripts only check those external
system dependencies; they do not install Docker, Python, OS packages, or other
third-party system software.

The repository's [Hermit](https://cashapp.github.io/hermit/) environment supplies
the project toolchain. The current pins are Rust 1.95.0, Node.js 24.14.0, and
pnpm 11.4.0. Their first use downloads the pinned Carryforth build tools and the
build installs Carryforth's own package dependencies.

For a fresh clone, the supported one-command local build and startup is:

```bash
git clone https://github.com/lgYanami/Carryforth.git
cd Carryforth
./start.sh
```

The first run checks the external prerequisites, creates a private Git-ignored
`.env` with mode `0600`, and asks for any missing semantic Provider API key,
Base URL, and Request Model. The API key uses hidden terminal input; the URL and
model are visible and have no defaults. Local source startup defaults the
semantic Worker and semantic query HTTP process switches to enabled and
generates a stable local Relay signer. It then downloads/builds Carryforth dependencies, starts the
development infrastructure without deleting volumes, applies pending database
migrations, builds the Relay/CLI/Desktop, and launches Desktop. The
Relay listens on `ws://localhost:3000`.

These startup defaults do not silently enable a Community's durable semantic
index/query gates and do not acknowledge problem egress. Those authorization
steps remain explicit. To opt out of Provider configuration entirely, set both
`BUZZ_SEMANTIC_WORKER_ENABLED=false` and
`BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=false` in `.env`.

For a foreground contributor workflow, activate Hermit and run `just dev`; it
uses the same semantic configuration step:

```bash
. ./bin/activate-hermit
just dev
```

For a detached workflow:

```bash
./start.sh                      # build and start without deleting data
./scripts/dev-rebuild-start.sh  # rebuild executables, then start
./scripts/dev-stop.sh           # stop processes/containers; preserve data
```

Do not use destructive reset commands against a development environment whose
data matters. Migration and destructive integration tests must target a
separately created scratch database; see [CONTRIBUTING.md](CONTRIBUTING.md).

### Agent CLI

Build the CLI and point it at the local Relay:

```bash
cargo build --release -p carryforth-cli
export CARRYFORTH_RELAY_URL=ws://localhost:3000
export CARRYFORTH_PRIVATE_KEY=<local-development-key>
./target/release/cf --help
```

The ACP harness injects the required `CARRYFORTH_*` variables into managed
agent subprocesses. Never paste production keys into issues, logs, documents,
or command examples.

## Architecture at a glance

```text
┌──────────────────────────────────────────────────────────────────┐
│ Carryforth Desktop     Managed agents / ACP       cf CLI         │
└───────────────┬──────────────────┬──────────────────┬────────────┘
                └──────────────────┼──────────────────┘
                                   │ signed Nostr events
                                   ▼
                         Local Carryforth Relay
                         (internal: buzz-relay)
                                   │
                 ┌─────────────────┼─────────────────┐
                 ▼                 ▼                 ▼
              Postgres           Redis           S3/MinIO
```

The Relay is the canonical state boundary. Messages, governance objects,
documents, context edges, meeting state, and audit records are community
scoped. See [ARCHITECTURE.md](ARCHITECTURE.md) for protocol and subsystem
details.

Many internal crate, database, event-kind, and capability identifiers retain a
`buzz-*` name for wire/storage compatibility. They are technical coordinates,
not the current product or repository identity. They will not be mechanically
renamed without a separately reviewed migration.

## Repository map

```text
crates/                 Relay, ACP, cf, protocol, storage, and tooling crates
desktop/                Supported Tauri + React desktop client
migrations/             Forward-only Relay database migrations
scripts/                Development and release tooling
deploy/                 Deployment sources under release-readiness review
web/                    Source-only browser client
docs/                   Design, operations, and development records
```

## Contributing and support

- Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.
- Use GitHub issues for reproducible bugs and scoped feature proposals.
- Do not disclose vulnerabilities in public issues; follow
  [SECURITY.md](SECURITY.md).
- Project decisions and maintainer responsibilities are described in
  [GOVERNANCE.md](GOVERNANCE.md).

## License

Carryforth is distributed under the [Apache License 2.0](LICENSE). The license
and upstream copyright notices remain intact. Third-party components and assets
may carry their own licenses; their current audit boundary is recorded in the
[release asset inventory](release/THIRD_PARTY_ASSETS.md) and must be cleared
before the first stable binary release.
