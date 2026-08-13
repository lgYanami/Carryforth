# Contributing to Carryforth

Thank you for helping improve Carryforth. The project is under active
development and currently supports source-based local development and
evaluation; it does not yet promise a stable binary release, production
deployment, or upgrade-support surface.

Questions and scoped proposals belong in the
[Carryforth issue tracker](https://github.com/lgYanami/Carryforth/issues).
Do not put vulnerabilities, credentials, private project content, or sensitive
conduct reports in a public issue. See [Security and private reporting](#security-and-private-reporting)
and the [Code of Conduct](CODE_OF_CONDUCT.md#enforcement) for their distinct
reporting paths.

## Code of Conduct

Participation is governed by the [Contributor Covenant](CODE_OF_CONDUCT.md).
Please keep technical disagreement focused on the work and preserve the privacy
of project data, identities, and reports.

## Security and private reporting

Use the repository's private vulnerability-reporting form for suspected
security vulnerabilities, including vulnerabilities that could expose private
project data:

<https://github.com/lgYanami/Carryforth/security/advisories/new>

For a new clean-history repository, make the otherwise-empty repository public,
enable and anonymously verify that form, and only then push the public source
history. If it is unavailable, do not disclose sensitive details in an issue; use
[GitHub Support](https://support.github.com/contact) for GitHub-hosted abuse and
open a public issue titled `Private vulnerability reporting is unavailable`
with no vulnerability details, reproduction steps, logs, or attachments. That
issue only asks the maintainer to restore the private channel. Do not use this
form for ordinary moderation disputes. The project
does not currently publish a separate security email address or response-time
SLA. Full reporting guidance is in [SECURITY.md](SECURITY.md).

## Development environment

### System prerequisites

Install these external prerequisites yourself:

- Docker 24 or newer with Docker Compose v2 and a running daemon;
- Python 3;
- `curl` for local readiness checks;
- the native Tauri dependencies for your operating system.

The startup scripts check these dependencies but do not install Docker, Python,
`curl`, OS packages, or Xcode tools. The repository uses
[Hermit](https://cashapp.github.io/hermit/) to pin Rust, Node.js, pnpm, `just`,
and other project tools. Activate it once per shell:

```bash
. ./bin/activate-hermit
```

Hermit downloads pinned tools on first use. If you choose a system toolchain
instead, it must be compatible with the versions declared under `bin/`, the
workspace manifests, and `package.json`; using an unpinned `just latest` is not
the reproducible path.

### First source build

The shortest supported local path is:

```bash
git clone https://github.com/lgYanami/Carryforth.git
cd Carryforth
./start.sh
```

The script initializes a Git-ignored `.env` with mode `0600`, starts the local
development services, applies forward migrations, builds the Relay, CLI and
Desktop, and starts the application. It prompts for a semantic Provider API
key, Base URL and request model because the semantic Worker and query HTTP
process switches are enabled for a fresh local environment. None of those
Provider values has a default. Disable both process switches in `.env` if you
do not want semantic capabilities.

The development stack uses development credentials. The checked-in Relay and
Compose examples publish their listeners on host loopback, but this is not a
production hardening profile. Run it only on a trusted local machine; see
[local development](docs/en/local-development.md).

For the split contributor workflow:

```bash
. ./bin/activate-hermit
just setup
just hooks
just dev
```

Useful lifecycle commands are:

```bash
./start.sh
./scripts/dev-rebuild-start.sh
./scripts/dev-stop.sh
```

These preserve local data. `just reset` is destructive and removes the local
development state and volumes described by the recipe; do not use it on data
that matters.

## Repository map

- `crates/`: Relay, persistence, protocol/domain libraries, Agent harnesses,
  `cf`, and supporting Rust binaries.
- `desktop/`: Tauri and React Desktop application. Its Rust crate is outside
  the root Cargo workspace.
- `web/` and `admin-web/`: browser-facing development clients.
- `migrations/` and `schema/`: upgrade and fresh-schema contracts.
- `docs/cn/` and `docs/en/`: current product and design explanations.
- `docs/stage/`: detailed specifications, implementation plans, qualification
  reports, and retained historical design records. Read each document's status.
- `deploy/`, `release/`, and `scripts/`: local deployment candidates, evidence
  manifests, checks, and developer tooling.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the current high-level system model
and [AGENTS.md](AGENTS.md) for repository-specific coding rules.

## Validation

Choose checks proportional to the change and state what you did not run.

```bash
just test-unit       # no external services
just test            # unit + integration; uses local Postgres and Redis
just ci              # full local PR gate defined by the current Justfile
```

`just ci` is the repository's broad local gate: formatting and clippy checks,
source-surface checks, Rust unit tests, Desktop checks/tests/build, Tauri
checks/tests, and the web build. GitHub Actions consists of multiple jobs and
may run additional platform, security, integration, or release-surface checks;
`just ci` is not a byte-for-byte reproduction of every hosted job.

Useful scoped commands include:

```bash
just desktop-check
just desktop-test
just desktop-tauri-check
just desktop-tauri-test
just web-check
just web-build
```

The Tauri Rust crate is excluded from the root Cargo workspace, so root
`cargo test` does not test it. Use its `just` recipes or manifest explicitly.
Relay-backed E2E tests live under `crates/buzz-test-client/tests/`; consult
[TESTING.md](TESTING.md) before running them because the default development
stack is shared with Desktop and is not an isolated disposable fixture.

`just fix-all` applies repository formatters and frontend autofixes. It does not
automatically repair every Rust clippy diagnostic; inspect and resolve those
deliberately.

## Engineering rules

The normative contributor rules are in [AGENTS.md](AGENTS.md). In particular:

- do not add `unsafe` Rust; a few inherited or platform/FFI boundaries contain
  reviewed existing `unsafe`, which is not permission to expand it;
- do not add `unwrap()` or `expect()` in production paths;
- preserve host-derived Community boundaries and fail closed across tenants;
- define protocol event kinds in `crates/buzz-core/src/kind.rs`;
- prefer signed Nostr operations over endpoint-specific HTTP APIs;
- keep migrations, `schema/schema.sql`, readiness, and drift tests aligned;
- put new Agent-facing project operations in `crates/carryforth-cli`;
- never commit `.env`, private keys, Provider credentials, project content, or
  internal infrastructure coordinates.

Use `rustfmt`, Biome, and the existing `just` recipes rather than inventing a
parallel style or build workflow. Document new public Rust APIs and prefer typed
errors over stringly typed failure paths.

## Making a pull request

Before starting a substantial change, check existing issues and pull requests
and open a scoped design discussion when the behavior or compatibility contract
is unclear. Small corrections may be submitted directly.

A useful pull request is:

1. focused on one coherent change;
2. covered by tests appropriate to its risk;
3. documented where it changes public behavior, configuration, protocol, or
   operational expectations;
4. explicit about checks not run and follow-up work;
5. free of secrets, private project data, and unrelated generated artifacts.

Suggested checklist:

```text
- [ ] Checks proportional to the change pass; anything omitted is explained
- [ ] New behavior has tests, or the reason tests are impractical is documented
- [ ] Public APIs/configuration/protocol changes are documented
- [ ] No new production unwrap/expect or unsafe code
- [ ] Database changes keep migration, fresh schema, readiness and drift aligned
- [ ] No credentials, private data or internal-only dependencies are included
```

The project is maintained on a best-effort basis. It does not promise a review
time. Maintainers may merge, squash, rebase, request changes, or decline a
change according to its scope and the repository's current state.

## Extending protocol and HTTP surfaces

Do not assume every feature requires a new event kind, and do not assume a new
kind is sufficient by itself. Carryforth combines signed Nostr events with
domain reducers, database projections, generic HTTP query/ingest surfaces,
specialized HTTP protocols where necessary, and Desktop-only presentation.

When adding an event kind:

1. register it and document its range in
   `crates/buzz-core/src/kind.rs`;
2. define and validate its closed payload/tag contract in the owning domain;
3. map admission scope and host-derived Community handling in the Relay;
4. keep reducers, projections, lifecycle/currentness checks, audit behavior,
   and side effects consistent;
5. update migrations/fresh schema if persistence changes;
6. add protocol, authorization, tenant-isolation, and readback tests;
7. update user/operator documentation where applicable.

Event storage does not imply that every kind is included in every audit or
search surface. PostgreSQL full-text search uses the generated
`events.search_tsv` contract and explicit privacy exclusions; semantic indexes
are separate derived data with their own gates.

Prefer the existing WebSocket and generic `POST /events`, `POST /query`, and
`POST /count` bridge. Endpoint-specific HTTP remains appropriate for genuinely
HTTP-native surfaces such as NIP metadata, health probes, Blossom media,
webhooks, and Git smart HTTP. Any new protected endpoint must resolve its
host-derived Community before tenant data access, authenticate according to its
surface, preserve authorization/currentness checks, and include negative
cross-Community tests.

## Repository and upstream

[`lgYanami/Carryforth`](https://github.com/lgYanami/Carryforth) is the canonical
public source repository. It must remain understandable and buildable without
Block organization access, private DNS, registries, or release infrastructure.

Carryforth began from an Apache-2.0-licensed Buzz source snapshot. Preserve
copyright, license, and provenance notices when modifying inherited code; see
[UPSTREAM.md](UPSTREAM.md). Existing `buzz-*` crates, database names,
environment variables, event tags, paths and bundle identifiers may be wire,
storage, or data-continuity contracts and must not be mechanically renamed.

## Contribution licensing

Carryforth is licensed under the [Apache License 2.0](LICENSE). By submitting a
contribution, you agree to license it under Apache-2.0 and affirm that you have
the right to submit it. There is currently no separate Contributor License
Agreement process. If an employer or another party may own rights in your work,
obtain the necessary permission before contributing.
