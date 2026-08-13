# AGENTS.md — Carryforth Agent Contributor Guide

This file contains repository-specific rules for AI coding agents. General
setup, architecture, testing, and release documentation lives in the linked
guides at the end; do not duplicate those guides here.

## Repository boundary

[`lgYanami/Carryforth`](https://github.com/lgYanami/Carryforth) is the
canonical public repository. It must build and remain understandable without
Block organization access, internal DNS, private registries, or private release
pipelines.

Carryforth is derived from the Apache-2.0-licensed
[`block/buzz`](https://github.com/block/buzz). Treat that repository as an
upstream provenance source, not as Carryforth's release or support endpoint.
Preserve [LICENSE](LICENSE), [NOTICE](NOTICE), and [UPSTREAM.md](UPSTREAM.md).

Existing `buzz-*` crate and binary names, environment variables, event tags,
database identifiers, storage paths, and bundle coordinates may be compatibility
contracts. Do not mechanically rename them. New user-facing names should use
Carryforth; compatibility identifiers require an explicit migration design.

## Before changing code

Activate the repository toolchain before running Git, hooks, or project tools:

```bash
. ./bin/activate-hermit
```

- Inspect `git status` first and preserve unrelated user changes.
- Keep changes within the requested scope; do not opportunistically rewrite
  compatibility surfaces.
- Prefer existing scripts and `Justfile` recipes over ad hoc commands.
- Never copy secrets, private infrastructure coordinates, or internal-only
  dependencies into source, fixtures, logs, or documentation.

The main code areas are:

- `crates/`: Relay, Nostr types, persistence, agent harnesses, CLI, and shared
  Rust libraries.
- `desktop/`: Tauri 2 and React 19 desktop application.
- `web/` and `admin-web/`: browser clients.
- `migrations/` and `schema/`: database upgrade and fresh-install paths.
- `deploy/`, `release/`, and `scripts/`: local deployment candidates, deferred
  packaged-artifact evidence, and developer tooling.

## Non-negotiable engineering rules

- Do not add `unsafe` Rust.
- Do not add `unwrap()` or `expect()` in production paths; propagate typed
  errors with `?`.
- Document new public APIs.
- Keep host-derived community boundaries intact. Tenant context must not be
  derived from client-controlled event tags.
- Prefer Nostr events over new endpoint-specific HTTP APIs. HTTP is reserved
  for surfaces that genuinely require it, such as Blossom media, webhooks, Git
  smart HTTP, NIP metadata, health probes, and the generic event/query/count
  bridge.
- Define every event kind in `crates/buzz-core/src/kind.rs` before adding its
  handling.
- Scope channel operations with NIP-29 `h` tags, not `e` tags.
- Relay filters must include explicit `kinds`; open-ended filters hit the
  relay p-gate.
- Code that inserts replies must maintain the root event's `reply_count` and
  `descendant_count`.
- Put new agent-facing operations in `crates/carryforth-cli`.
  `buzz-dev-mcp` remains the shell/file tool server for `buzz-agent`.
- Keep `evalexpr` workflow conditions small and directly testable.
- Database changes must keep migrations, `schema/schema.sql`, readiness
  checks, and relevant drift tests aligned.

## Validation

Run checks proportional to the change and report anything not run:

```bash
just test-unit       # Rust unit tests; no services
just test            # integration suite; requires Postgres and Redis
just ci              # full local PR gate
```

Useful scoped gates:

```bash
just desktop-check
just desktop-test
just desktop-tauri-check
just desktop-tauri-test
just web-check
just web-build
```

Use `just fix-all` for repository formatting and frontend autofixes; it does
not automatically resolve every Rust clippy diagnostic. Hooks are
installed by `just setup`; reinstall them with `just hooks` after toolchain
changes. Let hooks run their repository commands instead of rewriting them to
work around an unactivated shell.

The Tauri crate is excluded from the root Cargo workspace. Root `cargo test`
does not test it; use `just desktop-tauri-test` or its manifest explicitly.

## Relay and CLI conventions

`cf` is the agent-first Carryforth CLI:

```bash
cargo build --release -p carryforth-cli
./target/release/cf --help
```

- Managed agents receive `CARRYFORTH_RELAY_URL`,
  `CARRYFORTH_PRIVATE_KEY`, and `CARRYFORTH_AUTH_TAG`. Never print or echo
  private keys.
- `--format compact` is a global option: use
  `cf --format compact channels list`, not a subcommand-local flag.
- A `carryforth://message?channel=<uuid>&id=<hex>` link can be read with
  `cf messages thread --channel <uuid> --event <hex>`.
- `cf messages search` must include `--kinds`; for ordinary message search,
  use at least `9,45001,45003`.
- Read and write response contracts are documented in
  [`crates/carryforth-cli/TESTING.md`](crates/carryforth-cli/TESTING.md).

## Desktop and web rules

Desktop features live under `desktop/src/features/`; shared code belongs under
`desktop/src/shared/`. Biome owns TypeScript/React formatting.

### Text and zoom

Readable desktop text must use named, rem-based Tailwind tokens. Prefer stock
tokens such as `text-base`, `text-sm`, and `text-xs`; use `text-2xs` or
`text-3xs` for smaller metadata. Do not add arbitrary pixel or rem text sizes.
If the scale lacks a necessary size, add a named rem token to
`desktop/tailwind.config.js`. The `check:px-text` gate enforces this.

### Community-scoped state

Community switching remounts the React tree but does not reset module-level
state. Any module-level cache, `Map`, singleton, or pending promise that holds
community data must have a reset function wired into
`resetCommunityState()` in
`desktop/src/features/communities/useCommunityInit.ts`. This prevents data
from one relay from leaking into another.

When diagnosing render performance, remember that `React.memo` only helps
when every prop is reference-stable. React Query result objects and freshly
derived arrays, maps, callbacks, or JSX commonly defeat memoization.

### Desktop E2E and screenshots

- The desktop UI requires the mock Tauri bridge; do not treat a plain-browser
  render as valid evidence.
- Use `just desktop-screenshot --name <name> [options]` for simple captures.
- In custom Playwright specs, install `addInitScript` state before
  `installMockBridge(page)`.
- Wait for a mock live subscription before injecting live messages.
- Call the shared `waitForAnimations(page)` helper before every screenshot.
- Scope captures to the subject and compare hashes when producing multiple
  states; identical hashes indicate invalid duplicate evidence.
- Do not host PR screenshots through relay media. Validate hand-written
  Markdown with `scripts/check-pr-image-urls.sh`, then use
  `scripts/post-screenshots.sh` against the current Carryforth fork.

## Common pitfalls

- Channel metadata is kind `39000`, not kind `41`.
- Shell working directories do not persist between agent tool calls; provide
  the working directory explicitly or change directory in the same command.
- In Git worktrees, desktop Tauri formatting may need to run from the main
  checkout before restaging.
- Playwright may reuse a stale server. Rebuild and stop the process on port
  4173 before diagnosing a supposedly unchanged UI.
- The mock `general` channel contains seeded messages and is unsuitable for
  testing a clean no-unread state; use `engineering`.

## Reference documentation

- [CONTRIBUTING.md](CONTRIBUTING.md): setup, code style, PR process, and feature
  extension patterns.
- [TESTING.md](TESTING.md): integration and multi-agent E2E testing.
- [ARCHITECTURE.md](ARCHITECTURE.md): components, protocols, and data flow.
- [SECURITY.md](SECURITY.md): security model and reporting policy.
- [README.md](README.md): product overview and quick start.
