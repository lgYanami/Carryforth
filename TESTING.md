# Testing

## Automated Tests

```bash
just test-unit          # unit tests — no infrastructure needed
just test               # unit + integration (starts Docker if needed)
```

`just test` runs unit tests plus integration tests against Postgres and Redis.
It starts or reuses the repository's default development services; that stack
is shared with Desktop and is not disposable. Neither task runs the E2E suites
in `buzz-test-client` — those are marked `#[ignore]` and require a running relay:

```bash
# Start a relay first (see below), then:
cargo test -p buzz-test-client -- --ignored
```

---

## Live Local Relay

The fastest way to exercise the relay end-to-end is to build the release
binaries once, run `buzz-relay`, and drive it with the `cf` CLI. The HTTP
requests used in the smoke flow below are signed with NIP-98, so you don't need
`nak` or hand-rolled `curl` for those requests. Other `cf` surfaces use their
own protocol authentication contracts.

### 1. Setup

```bash
. ./bin/activate-hermit          # activate pinned toolchain
cp .env.example .env             # one-time
just setup                       # start Docker services, run migrations
```

> **Already running Carryforth Desktop?** Desktop uses the same Docker container
> names (`buzz-postgres`, `buzz-redis`) and the same
> default ports (`:5432`, `:6379`). `just setup` will reuse those
> services, so **your test relay writes into Desktop's database**. That's
> fine for an intentional read/write smoke test, but `just reset` wipes
> Desktop's data along with yours. Setting only `COMPOSE_PROJECT_NAME` does
> **not** isolate this repository's default Compose file: it contains fixed
> container, network, and volume names. For disposable work, use the dedicated
> `docker-compose.harness.yml` and `scripts/start-isolated-test-relay.sh`, or a
> complete override with different containers, ports, networks, and volumes.
> The checked-in harness is a **singleton**, not a parallel-checkout facility:
> its Compose project, loopback ports, volumes, and tmux session are fixed. It
> fails before startup if that project/session or any required port is active.
> It preflights Docker/Compose, the pinned Cargo and `pgschema`, tmux, Python,
> `curl`, and `lsof` before starting Compose. It does not drop the schema or
> volumes by default, although applying the current schema and test seed still
> mutates this disposable database. Only the explicit `--reset-database` option
> drops its database schema, and only `docker compose -p
> buzz-harness -f docker-compose.harness.yml down -v` deletes its dedicated
> volumes. Never use either destructive option on data that matters.

`just reset` wipes all local data and starts over — **including Carryforth
Desktop's data** if its services are sharing your dev stack (see callout
above).

> **Heads up — scrub stale env first.** If your shell inherits any of
> `CARRYFORTH_AUTH_TAG`, `CARRYFORTH_RELAY_URL`, or `CARRYFORTH_PRIVATE_KEY` from a
> prior session (or a staging config), `unset` them before continuing.
> A stale `CARRYFORTH_AUTH_TAG` fails the **local dev relay** with
> `auth_error: signature verification failed` on the first CLI write —
> it is *not* tolerated.
> ```bash
> unset CARRYFORTH_AUTH_TAG CARRYFORTH_RELAY_URL CARRYFORTH_PRIVATE_KEY
> ```

### 2. Build the binaries

```bash
cargo build --release -p buzz-relay -p carryforth-cli -p buzz-admin
export PATH="$PWD/target/release:$PATH"
```

Rebuild after any code change — the steps below use the release binaries.

### 3. Start the relay

In a separate terminal, load the same private `.env` that `just` uses, then
run the Relay in the foreground:

```bash
. ./bin/activate-hermit
set -a
source .env
set +a
export PATH="$PWD/target/release:$PATH"
buzz-relay                     # release binary from step 2, serves ws://localhost:3000
# alternatives:
# cargo run --release -p buzz-relay     # rebuild + run in release
# just relay                            # DEBUG build — fast to launch on a hot cache,
#                                       # but mismatched if step 2 left you on release.
#                                       # Use `just relay-release` if you want the recipe.
```

Verify it's up (back in your working terminal):

```bash
curl -s http://localhost:3000/health           # → ok
curl -s http://localhost:8080/_readiness        # → {"status":"ready"}
```

> Health/readiness/liveness live on a **separate port** (default `8080`,
> `BUZZ_HEALTH_PORT`) so probes bypass user-facing auth middleware. The main app
> port also exposes `/health` for convenience.

The checked-in `.env.example` binds the Relay listener to loopback, and the
commands above deliberately load that file. The raw binary also defaults to
`127.0.0.1:3000` when `BUZZ_BIND_ADDR` is absent. The checked-in Compose file
publishes dependency ports on loopback with development
credentials. The Relay starts in dev mode (`BUZZ_REQUIRE_AUTH_TOKEN=false` and
membership admission disabled). Run this stack only on a trusted local machine.
The startup log emits a warning about dev authentication; see the variables
below before deliberately exposing any listener.

> **Already running Carryforth Desktop (or another Relay) on `:3000` / `:8080` /
> `:9102`?** Carryforth Relay binds three ports — main, health, metrics — and any of
> them can collide. Use a separate terminal per role and export the right
> vars in each:
>
> **In the relay terminal** (before launching `buzz-relay`):
> ```bash
> export BUZZ_BIND_ADDR=127.0.0.1:3030
> export BUZZ_HEALTH_PORT=8088
> export BUZZ_METRICS_PORT=9202
> export RELAY_URL=ws://localhost:3030     # advertised in NIP-42 challenges
> buzz-relay
> ```
>
> **In your working / CLI terminal** (for steps 4+ and the ACP harness):
> ```bash
> export CARRYFORTH_RELAY_URL=http://localhost:3030    # CLI target
> # verify the relay on the overridden ports:
> curl -s http://localhost:3030/health             # → ok
> curl -s http://localhost:8088/_readiness         # → {"status":"ready"}
> ```
>
> Every snippet later in this doc shows the defaults. When you see
> `localhost:3000` / `:8080` in a code block, mentally substitute your
> overrides — or the CLI will end up talking to Carryforth Desktop's Relay.

> **Ignore `just setup`'s "Next steps" banner.** It still prints
> `just relay` (a debug build). Use `buzz-relay` from step 2 here —
> step 2 already built the release binary.

When you're done, stop the relay with Ctrl-C in its terminal. For a managed
`./start.sh` run, use `./scripts/dev-stop.sh`. If a manually launched terminal
was lost, identify the exact listener PID with `lsof`, verify its executable and
working directory belong to this checkout, and signal that PID. Do not use a
broad `pkill -f` pattern on a machine with parallel development checkouts.

### 4. Smoke test the CLI against the relay

End-to-end: generate an identity, create a channel, post a message, read it
back. This is the minimum sequence an agent needs to verify a local relay.

```bash
# Generate a keypair
GEN=$(buzz-admin generate-key)
export CARRYFORTH_PRIVATE_KEY=$(echo "$GEN" | awk '/Secret key:/ {print $3}')
PUBKEY=$(echo "$GEN"           | awk '/Public key:/ {print $3}')
echo "pubkey: $PUBKEY"

# Create a channel — the UUID is returned in the response
CHANNEL=$(cf channels create --name "smoke-$$" --type stream --visibility open | jq -r '.channel_id')
echo "channel: $CHANNEL"

# Send a message and read it back
SEND=$(cf messages send --channel "$CHANNEL" --content "hello from smoke test")
EVENT_ID=$(echo "$SEND" | jq -r '.event_id')
cf messages get --channel "$CHANNEL" --limit 5 | jq .

# Fetch the reply chain for a specific message (empty array on a leaf — that's fine)
cf messages thread --channel "$CHANNEL" --event "$EVENT_ID" | jq .
```

A successful run prints `{"event_id":"…","accepted":true,"message":""}` for
the send, and the message body in the `get` output. `thread` returns `[]`
for a leaf message — populated only after a reply comes in (see §5).

### 5. Going deeper

For the current CLI command matrix and response contracts, follow
[`crates/carryforth-cli/TESTING.md`](crates/carryforth-cli/TESTING.md). Avoid
copying command counts into this document because the Agent-first surface is
still changing.

The generic Nostr HTTP bridge exposes these three endpoints. They are not the
Relay's complete HTTP surface:

| Endpoint        | Purpose                            |
|-----------------|------------------------------------|
| `POST /events`  | Submit a signed Nostr event        |
| `POST /query`   | NIP-01 filter query (returns events) |
| `POST /count`   | NIP-45 count query                 |

Protected use should authenticate with NIP-98. The local dev configuration can
also allow an `X-Pubkey` fallback and must therefore remain loopback-only.
There is no endpoint-specific API for fetching message threads — use
`POST /query` with an `#e` filter, or `cf messages thread`.

---

## ACP Harness (optional, end-to-end with a real agent)

`buzz-acp` connects an ACP-speaking agent (goose, codex, claude code,
buzz-agent) to the relay. The harness listens for events, drives the
agent over stdio, and the agent replies through MCP tools.

Minimum recipe — assumes the relay from step 3 is running and the channel
`$CHANNEL` from step 4 still exists. The agent identity must be **different**
from the sender identity (`BUZZ_ACP_RESPOND_TO=anyone` still skips events
the agent signed itself).

```bash
cargo build --release -p buzz-acp
export PATH="$PWD/target/release:$PATH"

# 1. Save your sender identity from step 4 — you'll need it to @mention the agent
SENDER_SK="$CARRYFORTH_PRIVATE_KEY"

# 2. Mint a fresh agent identity and capture its pubkey
AGENT_GEN=$(buzz-admin generate-key)
AGENT_SK=$(echo "$AGENT_GEN" | awk '/Secret key:/ {print $3}')
AGENT_PUBKEY=$(echo "$AGENT_GEN" | awk '/Public key:/ {print $3}')

# 3. Add the agent as a member of $CHANNEL — still using the sender identity.
#    Skip this and the agent boots to "discovered 0 channel(s) → agent will
#    sit idle" and silently ignores every mention.
cf channels add-member --channel "$CHANNEL" --pubkey "$AGENT_PUBKEY" --role member

# 4. Switch to the agent identity and start it.
#    buzz-acp wants ws:// (not http://). If you set CARRYFORTH_RELAY_URL to an
#    http:// URL in step 3, set the ws:// equivalent here — same host/port.
export BUZZ_PRIVATE_KEY="$AGENT_SK"
export BUZZ_RELAY_URL=ws://localhost:3000   # match step 3 (e.g. ws://localhost:3030 if overridden)
export BUZZ_ACP_RESPOND_TO=anyone           # default is owner-only; opens the gate for testing
# NIP-AE core-memory prompt injection is on by default; set BUZZ_ACP_NO_MEMORY=true to opt out.
export GOOSE_MODE=auto                        # must be 'auto' or goose hangs on prompts

buzz-acp                                    # foreground; logs to stdout (run in a separate terminal)

# Optional: turn on per-turn tracing if the default log is too quiet.
# RUST_LOG=buzz_acp=debug buzz-acp
```

> **Using a different ACP agent?** The default recipe assumes `goose` is on
> `$PATH` and configured (`goose --version` should print). For codex / claude
> code / buzz-agent, set `BUZZ_ACP_AGENT_COMMAND` and `BUZZ_ACP_AGENT_ARGS`
> accordingly — see `crates/buzz-acp/README.md`. Without these, buzz-acp
> will fail to spawn the agent subprocess on startup.

If you started the agent before adding it to the channel, just run the
`add-member` afterwards — it picks up the membership notification live and
subscribes without restart (`membership notification: subscribing to new channel …`).

The justfile also ships `just goose key="$AGENT_NSEC"` (foreground) and
`just goose-bg key="$AGENT_NSEC"` (background screen session) which set the
same env. See `crates/buzz-acp/README.md` for parallel agents, heartbeats,
respond-to gates, and forum subscriptions.

To exercise deferred ACP startup, add `BUZZ_ACP_LAZY_POOL=true` before launching
`buzz-acp`. The harness should connect, authenticate, subscribe, and publish
online presence without starting the configured ACP child. The first accepted,
flushable mention should start exactly one child and then dispatch the queued
message. Automated coverage in `pool_lifecycle_state` pins single-wake,
retry/backoff, and stale-result behavior; it does not replace this real
relay/process smoke test.

Send the agent a task — switch your shell back to the **sender** identity
from step 4 and @mention the agent:

```bash
export CARRYFORTH_PRIVATE_KEY=$SENDER_SK    # the key from step 4
cf messages send --channel "$CHANNEL" \
  --content "Hey agent, reply PONG only."

# Wait 10–90s, then read the channel — the agent's reply is a kind:9 from
# AGENT_PUBKEY. The current ACP build is quiet on stdout during a turn, so
# `cf messages get` is how you confirm it ran.
cf messages get --channel "$CHANNEL" --limit 5 | jq '.[] | {pubkey, content}'
```

Replies are kind:9 in the same channel; `cf messages thread --channel <id>
--event <event_id>` fetches the reply chain for a specific mention.

---

## Configuration reference

The relay reads all configuration from environment variables. Defaults work
out of the box with `just setup` or `just relay`. Common overrides:

| Variable                          | Default                     | Notes |
|-----------------------------------|-----------------------------|-------|
| `BUZZ_BIND_ADDR`                | `127.0.0.1:3000` | Main app listener. Set it explicitly only after designing authentication and network controls for broader exposure. |
| `BUZZ_HEALTH_PORT`              | `8080`                      | `/_liveness`, `/_readiness` |
| `BUZZ_METRICS_PORT`             | `9102`                      | Prometheus `/metrics` |
| `RELAY_URL`                       | `ws://localhost:3000`       | Advertised in NIP-11 / NIP-42 challenges. **Note: no `BUZZ_` prefix.** |
| `DATABASE_URL`                    | `postgres://buzz:buzz_dev@localhost:5432/buzz` | |
| `REDIS_URL`                       | `redis://localhost:6379`    | |
| `BUZZ_REQUIRE_AUTH_TOKEN`       | `false`                     | When true, REST requires NIP-98 (no `X-Pubkey` fallback) |
| `BUZZ_REQUIRE_RELAY_MEMBERSHIP` | `false`                     | When true, only pubkeys in `relay_members` can connect |
| `BUZZ_REQUIRE_MEDIA_GET_AUTH`   | `false`                     | When true, `GET`/`HEAD /media/*` require Blossom kind 24242 `t=get` auth plus relay membership. |
| `BUZZ_AUDIT_ENABLED`            | `true`                      | Tamper-evident event/media audit log. Set `false`/`0`/`off` to skip its DB pool and writes. Does not disable the separate moderation audit trail. |
| `BUZZ_AUTO_MIGRATE`             | `false`                     | Opt in with `true`/`1`/`yes`/`on` to run embedded SQLx migrations on relay startup |
| `RELAY_OWNER_PUBKEY`              | unset                       | Bootstrapped as `owner` in `relay_members` at first start |
| `BUZZ_ALLOW_NIP_OA_AUTH`        | `false`                     | Enable NIP-OA owner attestation for membership |
| `BUZZ_WEB_DIR`                  | unset (source), `/srv/buzz/web` (container image) | Directory containing the invite landing bundle; packaged container configurations may set it so `/invite/{code}` works |
| `BUZZ_SERVE_GIT_WEB_GUI`        | `false`                     | Set to `true` or `1` to expose the bundled Git repository browser at `/` and `/repos/...`; invite routes do not depend on this flag |

CLI-side, these three matter for testing:

| Variable                | Default                  | Notes |
|-------------------------|--------------------------|-------|
| `CARRYFORTH_RELAY_URL`   | `http://localhost:3000`  | CLI relay base; accepts `ws(s)://` and normalises |
| `CARRYFORTH_PRIVATE_KEY` | — (**required**)         | `nsec1…` or 64-char hex |
| `CARRYFORTH_AUTH_TAG`    | unset                    | Optional NIP-OA owner attestation JSON |

---

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `relay error 500` or `400: restricted: not a channel member` after a code change | Stale binary | Rebuild and re-export `PATH`; or `cargo run` directly |
| `Address already in use` on relay start (os error 48 on macOS, 98 on Linux) | Another relay (or stale process) holding `:3000` / `:8080` / `:9102` (or your override ports) | Read the failing port, inspect it with `lsof -iTCP:3000,8080,9102 -sTCP:LISTEN`, and verify the exact PID/executable/checkout. Stop a managed instance with `./scripts/dev-stop.sh`; otherwise signal only the verified PID or choose fresh ports. |
| `auth_error: CARRYFORTH_PRIVATE_KEY is required` | Env not exported into the CLI's shell | `export CARRYFORTH_PRIVATE_KEY=...` (or pass `--private-key`) |
| `auth_error: CARRYFORTH_AUTH_TAG verification failed … signature verification failed` | A stale `CARRYFORTH_AUTH_TAG` inherited from a parent shell. The local dev relay rejects it. | `unset CARRYFORTH_AUTH_TAG` (see the scrub block in step 1) |
| `auth-required: verification failed` on a closed relay | NIP-OA attestation needed | Set `CARRYFORTH_AUTH_TAG` to the owner-issued JSON, or relax `BUZZ_REQUIRE_RELAY_MEMBERSHIP` |
| `channels list` empty after `channels create` | The create response includes `channel_id`, but the new identity may be querying a different Relay/Community or using stale credentials | Capture `.channel_id` as shown in step 4, verify `CARRYFORTH_RELAY_URL` and credentials, then query that Relay. |
| ACP agent ignores all events | `BUZZ_ACP_RESPOND_TO=owner-only` (default) with no owner configured | Set `BUZZ_ACP_RESPOND_TO=anyone` for testing |
| ACP logs `discovered 0 channel(s)` / `no channel subscriptions resolved` | Agent identity isn't a member of any channel | `cf channels add-member --channel "$CHANNEL" --pubkey "$AGENT_PUBKEY" --role member` from another identity |
| `GOOSE_MODE` warning, agent hangs | Not set | `export GOOSE_MODE=auto` |
| Tests pass locally but CI fails | Forgot to run `just ci` | `just ci` runs the gate (fmt, clippy, unit tests, desktop/web builds) |
