# Carryforth Local Development from Source

> This document describes the local source-build and startup workflow currently supported by the
> repository. It is not a production deployment guide, a stable installer guide, or an upgrade
> manual for existing data.

## 1. Scope

The recommended entry point is the root-level script:

```bash
./start.sh
```

It supports both:

- building and starting Carryforth from source for the first time;
- starting it again after a previous build or run while preserving existing data.

The script recognizes managed processes from the current checkout and existing Docker containers.
On repeated runs, healthy containers and build caches may be reused. It does not delete database
volumes, Desktop state, or user data in order to produce a “clean start.”

## 2. External prerequisites

Prepare these dependencies yourself before starting:

- Docker 24+;
- the Docker Compose v2 plugin;
- a running Docker daemon;
- Python 3;
- `curl`, used for local readiness checks;
- the native Tauri dependencies for the current platform.

On Linux, the startup script also checks `pkg-config` and Desktop dependencies such as WebKitGTK,
GTK, libsoup, ALSA, and appindicator. On macOS, it checks Xcode Command Line Tools.

The startup script only checks these external system dependencies. It **does not** automatically
install Docker, Python, `curl`, system packages, or Xcode tools.

The repository uses [Hermit](https://cashapp.github.io/hermit/) for a pinned project toolchain.
First use downloads the pinned Rust, Node.js, pnpm, and `just` versions. The build also downloads
Rust and frontend dependencies.

## 3. First start

```bash
git clone https://github.com/lgYanami/Carryforth.git
cd Carryforth
./start.sh
```

`start.sh` is a stable root entry point that delegates to `scripts/dev-start.sh`. The startup flow:

1. checks Docker, Compose, Python, `curl`, the Docker daemon, and platform-native dependencies;
2. activates the repository's Hermit toolchain;
3. creates or updates the Git-ignored local `.env` and sets its mode to `0600`;
4. checks semantic Provider configuration;
5. starts or resumes the development Docker Compose services without deleting volumes;
6. waits for PostgreSQL, Redis, MinIO, Keycloak, and Prometheus readiness;
7. runs `just dev` in a managed background process group;
8. applies pending forward migrations and builds Relay, CLI, and Desktop;
9. returns after Relay readiness and the Desktop process are ready.

Runtime state and logs are stored under:

```text
target/dev-lifecycle/
```

The default Relay address is:

```text
ws://localhost:3000
```

## 4. Semantic Provider configuration

The supported `./start.sh` and `just dev` paths enable all four semantic
**process switches** by default:

```dotenv
BUZZ_SEMANTIC_WORKER_ENABLED=true
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true
CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE=true
CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE=true
```

These switches host the background indexer, bounded complete-path query, natural-language
Coordinate discovery, and both one-hop directions. Relay capability advertisement and execution
still require every canonical-data and coverage readiness check to pass.

Whenever any semantic indexing or query process is enabled, one complete Provider setting family
with no defaults must be supplied explicitly. The source launcher prompts for it because all four
semantic processes start enabled by default:

| Preferred shared variable | Compatibility variable | Interactive input | Meaning |
|---|---|---|---|
| `LLM_API_KEY` | `BUZZ_SEMANTIC_API_KEY` | hidden | Provider API Key |
| `LLM_BASE_URL` | `BUZZ_SEMANTIC_BASE_URL` | visible | HTTPS Provider Base URL |
| `LLM_MODEL` | `BUZZ_SEMANTIC_REQUEST_MODEL` | visible | Embedding Request Model |

The two families cannot be mixed field by field. If any `BUZZ_SEMANTIC_*` Provider value is set,
the Relay treats that compatibility family as authoritative and requires it to be complete;
otherwise it uses the complete `LLM_*` family. This preserves deployed compatibility while letting
local agents and the semantic Provider share one connection configuration.

In an interactive terminal, the script asks for missing values. In a non-interactive environment,
it fails immediately and lists the missing variable names. It never guesses a Provider or fills in
a default URL or model.

Values are written only to the local Git-ignored `.env`. The API Key must not appear in terminal
echo, logs, documentation, issues, or test fixtures.

To avoid semantic capabilities entirely, keep all four process switches disabled in `.env`:

```dotenv
BUZZ_SEMANTIC_WORKER_ENABLED=false
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=false
CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE=false
CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE=false
```

### 4.1 Supported local semantic bootstrap

Running `./start.sh` (or `just dev`) is the local operator's authorization for the configured
Provider to receive the currently approved semantic inputs: problem/query text plus source type,
current visible title/name, and optional summary. The current foundation does not send Document
bodies or chunks.

For only the Community resolved from the loopback `RELAY_URL`, startup then:

1. reuses a compatible active generation, or creates one when none exists;
2. enables the durable semantic-index gate and performs a resumable canonical scan;
3. starts the Relay worker and waits for exact generation coverage;
4. activates the generation and arms the durable query gate;
5. verifies the live Relay reports the Worker and all three HTTP surfaces enabled.

The bootstrap command refuses non-loopback Relay, bind, or database coordinates and refuses a
multi-Relay fleet policy; those environments must use the normal operator workflow.
The operation is idempotent across restarts. It never sweeps other Communities. Raw Relay and
production startup remain capability-off unless an operator follows the normal deployment
contract in [Semantic pgvector operations](../semantic-pgvector-operations.md).
The worker-drain deadline defaults to 600 seconds and can be adjusted in `.env` with
`BUZZ_LOCAL_SEMANTIC_BOOTSTRAP_TIMEOUT_SECONDS` (1–3600); timeout fails startup instead of claiming
a partially initialized semantic stack is ready.

Startup does **not** create, decide, or sign Project View / Project Context state for the Human
Owner. When that canonical state is absent, the query gate is armed but existing authorization SQL
continues to reject retrieval and Relay capability advertisement remains off. After the Owner
initializes it, the already-running Worker indexes eligible state and the normal readiness fences
open the capabilities when coverage is complete.

Setting all four switches to `false` explicitly opts out and skips local semantic bootstrap. A
partial switch configuration is treated as an advanced/manual mode: the requested processes start,
but automatic full-capability Community bootstrap is skipped.

## 5. `just start`, `just dev`, and `./start.sh`

`Justfile` is the repository task entry point. It defines recipes for building, checking, testing,
starting, and stopping the system.

If Hermit is already active in the current shell, this is equivalent to the root entry point:

```bash
. ./bin/activate-hermit
just start
```

Both ultimately invoke `./start.sh`. New users should prefer `./start.sh` because it activates
Hermit itself.

For a foreground contributor workflow with visible build and service output:

```bash
. ./bin/activate-hermit
just dev
```

`just dev` is not a separate installer. It uses the same `.env`, Docker services, migrations, and
local Relay coordinates.

## 6. Rebuild and stop

```bash
./start.sh                      # build or reuse the build, then start in background
./scripts/dev-rebuild-start.sh  # clean Carryforth executable outputs, rebuild, and start
./scripts/dev-stop.sh           # stop application and Compose containers; preserve data
```

`dev-rebuild-start.sh` cleans only Carryforth's own executable build outputs. It preserves
dependency caches and Docker data. It is useful for excluding stale binaries or incremental-build
mismatches; it does not clear the entire Cargo or pnpm cache.

To stop only the application while leaving Docker containers running:

```bash
./scripts/dev-stop.sh --app-only
```

## 7. Local services and development boundary

The source-development Compose stack runs PostgreSQL/pgvector, Redis, MinIO, Keycloak, Prometheus,
Adminer, and related dependencies. It uses development configuration and publishes several ports
to the host. It is not a production-hardened deployment.

The checked-in `.env.example` and the raw Relay default both bind the Relay to
`127.0.0.1`. Keep that loopback boundary for ordinary source development:
local auth/membership gates are relaxed, and Compose dependency ports use
development credentials. The checked-in
Compose file binds those host ports to loopback. This is still not a production
hardening profile; run it only on a trusted local machine, and do not change the
bindings to expose it directly to a LAN or the Internet.

Desktop uses the local Relay and does not fall back to a legacy hosted service when that Relay is
unavailable. “Local-first” is not “fully offline”: Provider calls, toolchain and dependency
downloads, remote media, and external links may still access the network.

## 8. Data protection

- Do not use a destructive reset on an environment whose data matters.
- Do not run `docker compose down -v` or delete Carryforth development volumes.
- Do not manually rewrite an already-applied migration.
- Migration, OOM, fault-injection, and destructive integration tests must use a separate scratch
  database and volume.
- The checked-in development Compose stack is a machine-wide singleton: its project, containers,
  ports, network, and volumes are fixed and shared across checkouts. Do not run it concurrently
  from another checkout. Application-process stop logic is checkout-aware, but a normal
  `dev-stop.sh` also stops these shared Compose dependencies; use `--app-only` when appropriate.
- Never commit `.env`, private keys, or Provider credentials to Git.

## 9. Common checks

After changing code, run repository recipes proportional to the change:

```bash
. ./bin/activate-hermit
just test-unit
just desktop-check
just desktop-test
just desktop-tauri-check
just desktop-tauri-test
```

The full local PR gate is:

```bash
just ci
```

See [CONTRIBUTING.md](../../CONTRIBUTING.md) and [TESTING.md](../../TESTING.md) for additional
contribution and testing requirements.
