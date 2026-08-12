# Semantic pgvector deployment and upgrade

Carryforth semantic indexing requires PostgreSQL 17 with pgvector 0.8.5 installed in
the writer database. The extension is a database prerequisite; semantic
indexing itself remains disabled per Community until an operator explicitly
enables it.

The repository pins the official multi-architecture image as:

```text
pgvector/pgvector:0.8.5-pg17-bookworm
sha256:d2ef61f42ef767baa5a1475393303cc235bcd92febd9d7014eddb48b41f3bad0
```

## New local and quickstart databases

The repository development and supported local Compose files use the pinned
image and execute `CREATE EXTENSION IF NOT EXISTS vector` only while initializing
a new database volume. Existing volumes do not rerun init scripts.

Verify a development host independently with:

```bash
./scripts/test-semantic-pgvector.sh
```

The probe creates an isolated temporary container, checks a 2048-dimensional
full-precision vector, builds the planned `halfvec(2048)` HNSW expression
index, and runs the Rust/SQLx preflight. It does not use or alter the normal
Carryforth development database.

## Existing PostgreSQL volumes

Before changing an existing deployment:

1. take and verify a PostgreSQL backup;
2. confirm the replacement image uses PostgreSQL major 17, the same data
   directory, compatible UID/GID and locale, and includes pgvector 0.8.5;
3. stop writers and start the replacement image against a cloned volume first;
4. use a privileged database channel to run
   `CREATE EXTENSION IF NOT EXISTS vector` in the Carryforth database;
5. run `buzz-admin semantic preflight` with the ordinary Carryforth database role;
6. restart the old image against the cloned volume once to exercise the image
   rollback boundary before touching production;
7. retain the database backup until the semantic rollout observation window
   has closed.

Changing the container image does not upgrade the PostgreSQL data format. Do
not cross a PostgreSQL major version with an in-place image swap; use the
normal `pg_upgrade` or dump/restore process instead.

## External managed PostgreSQL

Use the provider's privileged extension-management path to install pgvector in
the exact writer database. Carryforth's preflight is deliberately read-only: seeing
an extension control file does not prove the runtime role may install it.

Run:

```bash
DATABASE_URL='postgres://…' buzz-admin semantic preflight
```

The command exits `0` only when PostgreSQL 17, pgvector 0.8.5, `vector`,
`halfvec`, cosine distance, the `vector`→`halfvec` cast, and the SQLx 2048-value
bind/decode path all satisfy the frozen contract. Exit `5` means a prerequisite
is unavailable. The JSON `failure_codes` are safe for automation.

## First semantic-schema rollout

For the first rollout, keep Relay automatic migration disabled with
`BUZZ_AUTO_MIGRATE=false` until the operator preflight succeeds.

Then perform this sequence:

1. install pgvector with a privileged database role;
2. run `buzz-admin semantic preflight` with the Carryforth runtime role;
3. run the packaged `buzz-admin migrate` exactly once;
4. verify the semantic schema while every Community capability remains off;
5. deploy Relay/worker code;
6. create and shadow-build a model generation for one explicitly approved
   Community;
7. enable broader operation only after coverage and currentness verification.

The semantic migration does not install or drop extensions and does not
backfill sources. Disabling semantic indexing never modifies Project View,
Project Document, Meeting, or Project Context canonical data.

## Provider worker configuration

The worker never falls back to generic `LLM_*` variables. Configure a
semantic-specific secret and explicit process switch:

```text
BUZZ_SEMANTIC_WORKER_ENABLED=true
BUZZ_SEMANTIC_API_KEY=<Volcengine Ark secret>
BUZZ_SEMANTIC_BASE_URL=https://ark.cn-beijing.volces.com/api/plan/v3/
BUZZ_SEMANTIC_REQUEST_MODEL=doubao-embedding-vision
BUZZ_SEMANTIC_REQUEST_INTERVAL_MS=1000
BUZZ_SEMANTIC_REQUEST_TIMEOUT_SECS=30
BUZZ_SEMANTIC_CLAIM_SECS=60
BUZZ_SEMANTIC_MAX_ATTEMPTS=8
```

The frozen first generation requires the provider response to resolve to
`doubao-embedding-vision-251215`, return exactly 2048 finite values, use cosine
distance, and use no client-side normalization. The one-request-per-second
default is enforced through a writer-PostgreSQL Community/provider rate gate
shared by every Relay worker replica, rather than once per process. Increase it only after queue age,
429 rate, request latency, and provider cost have been observed for the gray
Community.

Only `source type + title/name + optional source-owned summary` may cross this
provider boundary. The external adapter rejects `content_chunk`; enabling this
worker does not authorize Document bodies, Meeting Board/Speech, Project
Context topology, Role/Work lens data, or future chunks.

## First gray Community

Run every command with the deployment host/Community selected through the
normal `buzz-admin` tenant configuration. Do not use a production Community as
the first target.

```bash
# prerequisite and schema
buzz-admin semantic preflight
buzz-admin migrate

# immutable 2048-dimensional generation
buzz-admin semantic generation-create --volcengine

# explicit data-egress authorization for this Community
buzz-admin semantic enable

# durable all-family canonical scan; supplying the UUID up front makes resume
# independent of terminal/log retention
buzz-admin semantic rebuild \
  --generation-id <generation-uuid> \
  --operation-id <operator-generated-operation-uuid>

# wait for the enabled worker to drain, then prove completeness and cut over
buzz-admin semantic status
buzz-admin semantic verify --generation-id <generation-uuid>
buzz-admin semantic generation-ready --generation-id <generation-uuid>
buzz-admin semantic activate --generation-id <generation-uuid>
```

`verify` cannot pass on an unscanned empty catalog: a completed durable
all-family rebuild is an independent cutover fence. A source write invalidates
its old head synchronously, while encoding and activation remain asynchronous.
Activation qualifies the derived generation for graph query, but it does not
open a query route. Migration 0058 leaves both the per-Community query gate and
the HTTP deployment master disabled. The supported local Compose stack also
keeps graph query disabled; qualification currently requires a source-run
deployment with the additional settings below.

For immediate containment, run `buzz-admin semantic disable`. Currentness
capture continues while disabled, but no new source text is claimed or sent to
the provider. Re-enable only after resuming/re-running rebuild, draining jobs,
and verifying the generation. Retire and purge old generations only after the
rollback observation window; use `semantic gc` for unreferenced retired or
abandoned unit sets.

## Semantic graph-query qualification

### Supported local resource profile

The repository-supported semantic local profile uses the pinned PostgreSQL 17
/ pgvector 0.8.5 image with a 2 GiB container limit, `max_connections=40`,
`shared_buffers=256MB`, `work_mem=4MB`, `maintenance_work_mem=128MB`,
`effective_cache_size=1536MB`, and one parallel worker per gather. The Relay
uses explicit main/read/control/audit/search pool ceilings. Row-zero host
binding and readiness use the dedicated control pool; semantic Stage C uses a
separate fail-fast traversal admission gate and cannot consume the ordinary
main-pool reserve.

Before enabling graph queries, apply the current Compose profile without
removing or recreating the PostgreSQL volume, then run the read-only preflight:

```bash
just semantic-local-capacity-check
```

The check refuses remote Docker contexts and foreign containers, validates the
running PostgreSQL/pgvector/settings/migration/schema contract, verifies the
connection budget and traversal reserve, and prints only aggregate resource
evidence. It never changes settings, starts a query, enables a Community,
rebuilds an index, or prints database/provider credentials. A failure means the
query gate stays closed; never use `docker compose down -v` to apply this
profile.

`BUZZ_SEMANTIC_GRAPH_TRAVERSAL_MAX_IN_FLIGHT=2` is the provisional local
default. Saturation returns a retryable busy response before opening the Stage C
read transaction. It must be requalified in a disposable stack at 1, 2, 4, and
8 before raising the default; concurrency 1 is a smoke-test setting, not the
long-term product limit.

During PostgreSQL recovery, tenant-scoped row-zero reads return generic
retryable 503 instead of an incorrect permanent 404. NIP-11 remains available
and marks dynamic extension observation as `temporarily_unavailable`; current
Desktop/`cf` clients back off and re-verify. Only a healthy lookup that truly has
no host mapping returns 404.

Graph query is present but fail-closed by default. Enabling the HTTP deployment
master or choosing a Fleet policy does not enable Provider egress: each
Community still requires the explicit query gate and problem-egress
acknowledgement.

Carryforth currently supports local source builds with exactly one Relay. That
topology uses the default policy:

```text
BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY=trusted-single-relay
```

This policy skips only the short-lived load-balancer inventory assertion. It
does not skip database/schema readiness, Project Context capability, caller
authorization, current-source checks, Provider admission, or the final
pre-signing confirmation. It is not qualified for multiple Relays, a load
balancer, or production deployment.

Keep these gates closed while applying migration 0058 and preparing the
Foundation generation:

```text
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=false
BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY=trusted-single-relay
semantic_graph_query_enabled=false
```

Run the additive migration and inspect the closed state:

```bash
buzz-admin migrate
buzz-admin semantic preflight
buzz-admin semantic query-readiness
```

Without `--relay-status-url`, HTTP runtime fields come from the `buzz-admin`
process environment. The command labels that source in JSON and prints a
warning because those values do not describe an already-running Relay. To
observe the local Relay, use its loopback health endpoint:

```bash
buzz-admin semantic query-readiness \
  --relay-status-url http://127.0.0.1:8080/_status
```

Live status diagnostics accept only a literal loopback address and the exact
`/_status` path. They use no proxy or redirects, have short connection/request
timeouts, cap the response at 64 KiB, require the Relay schema and matching
compiled runtime digest, and fail without falling back to the admin process
environment. The status URL is read-only diagnostic evidence; it is never an
authorization source for `query-enable` or query execution. Because `/_status`
is deployment-global and the selected database tenant is Community-scoped, the
command does not claim that an arbitrary explicitly supplied status endpoint is
bound to that Community. It reports `community_binding_verified=false` and
keeps the legacy `base_enable_ready` field null; inspect
`database_and_policy_ready`, `http_runtime_ready`, and the source/scope fields
as separate diagnostic observations.

The database observation plus a bound operator configuration or explicit Live
status observation must together prove the active generation, exact non-zero
current heads, Project Context structural reads, provider/model contract,
stable Relay signer, virtual-kind storage constraint, and database
prerequisites. Environment-only `query-readiness` output does not by itself
prove a running handler. In local mode it reports:

```json
{
  "fleet_policy": "trusted-single-relay",
  "fleet_attestation_required": false,
  "fleet_attestation_status": "not_required"
}
```

An expired, revoked, or missing dormant Fleet row does not affect local query
admission. If an upgrade reports historical zero-vector heads, keep query
disabled and run:

```bash
buzz-admin semantic repair-query-vectors
buzz-admin semantic status
buzz-admin semantic query-readiness
```

Do not proceed until the worker has rebuilt every scheduled current head and a
second repair is a no-op. This repair does not modify canonical Project View,
Document, Meeting, or Project Context source data.

For the supported single-Relay local topology, start the Relay with the HTTP
master enabled, verify live readiness, and then explicitly authorize the
selected Community:

```text
BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true
BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY=trusted-single-relay
```

Run mutation commands from an operator shell or container that explicitly
loads the same protected `.env` / deployment configuration as the Relay.
`buzz-admin` does not load `.env` by itself, and `--relay-status-url` is
diagnostic only: it never fills mutation configuration from the observed
process.

```bash
buzz-admin semantic query-readiness \
  --relay-status-url http://127.0.0.1:8080/_status
buzz-admin semantic query-enable --acknowledge-problem-egress
```

Local `query-enable` still verifies the complete database prerequisites inside
the enabling transaction; it simply does not read the Fleet Attestation row.
The acknowledgement authorizes only the query `problem` plus current
source-owned title/summary overview to cross the configured semantic provider
boundary. It does not authorize Document bodies, chunks, Meeting Board/Speech,
topology, runtime hints, or other free text.

### Future multi-Relay qualification

Before introducing a second routable Relay, a load balancer, or a production
deployment, explicitly switch every instance and operator environment to:

```text
BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY=attested-fleet
```

Keep every Community query gate disabled, deploy all query-capable instances
with the same deployment identifier, and give each one a unique stable instance
identifier. Enumerate the exact instances currently routed by the load balancer,
then create and verify a short-lived assertion:

```bash
buzz-admin semantic fleet-attest \
  --inventory <current-lb-inventory.json> \
  --expires-in-seconds 300 \
  --acknowledge-current-routing-inventory
buzz-admin semantic fleet-check
buzz-admin semantic query-readiness
```

In `trusted-single-relay`, `fleet-check` instead returns
`applicable=false`, `status=not_required`, and exit code 0. `fleet-attest` and
`fleet-revoke` fail before opening the database, so they cannot mutate dormant
strict-mode state accidentally. To operate on that state, explicitly select
`attested-fleet` first.

Only after real-provider relevance, target-database latency/resource evidence,
policy-homogeneous routing, soak, and an approved first-Community canary may a
future multi-Relay operator enable the query gate:

```bash
buzz-admin semantic query-enable --acknowledge-problem-egress
```

Recheck readiness, Fleet expiry, and NIP-11 after enabling; only the HTTP
graph-query capability may be advertised. `attested-fleet` retains the existing
short lease and fails closed if the assertion expires, is revoked, does not
contain the serving instance, or has a different deployment/runtime digest.

## Graph-query rollback

In either policy, the immediate query-egress kill switch is:

```bash
buzz-admin semantic query-disable
buzz-admin semantic query-readiness
```

In `attested-fleet`, revoke the current assertion as an additional containment
step:

```bash
buzz-admin semantic fleet-revoke
```

`fleet-revoke` is deliberately unavailable in `trusted-single-relay`, where a
dormant Fleet row has no admission authority. Then remove affected instances
from routing or set
`BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=false`. Query rollback does not
delete canonical data, advance a business revision, delete the active semantic
generation, or stop ordinary Project Context reads. The Foundation indexing
worker may continue; disabling it is a separate operator decision. Never put an
older Relay that cannot parse the semantic raw extension back into routing
while a Community query gate or reusable strict Fleet assertion remains active.
