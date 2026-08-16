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

Local source startup enables these two **process switches** by default:

```dotenv
BUZZ_SEMANTIC_WORKER_ENABLED=true
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true
```

Agent-directed start discovery and one-hop semantic selection have independent process masters.
They remain off unless an operator explicitly adds them to the private `.env`:

```dotenv
CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE=false
CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE=false
```

Turning either one on uses the same Provider and semantic-index foundation; it still does not open
the durable Community gates or advertise a capability until the remaining readiness checks pass.

Whenever any semantic indexing or query process is enabled, one complete Provider setting family
with no defaults must be supplied explicitly. The source launcher prompts for it because its Worker and
complete-path Query switches start enabled by default:

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

### 4.1 Process switches are not Community authorization

Starting the Worker and Query HTTP handler only means that the local processes can host those
capabilities. It does not automatically perform:

- Project View / Project Context initialization;
- the durable Community semantic-index gate;
- generation creation, build, verification, or activation;
- the durable Community semantic retrieval/query gate;
- acknowledgement that the problem and overview text (source type, current visible title/name, and
  optional summary; not Document bodies/chunks in the current foundation) leave the local system
  for the external Provider.

An operator and Community owner must perform those steps explicitly under the current operations
contract. See [Semantic pgvector operations](../semantic-pgvector-operations.md).

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
