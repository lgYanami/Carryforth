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
The foundation release exposes no public semantic query, so activation only
qualifies the derived generation for the later query design.

For immediate containment, run `buzz-admin semantic disable`. Currentness
capture continues while disabled, but no new source text is claimed or sent to
the provider. Re-enable only after resuming/re-running rebuild, draining jobs,
and verifying the generation. Retire and purge old generations only after the
rollback observation window; use `semantic gc` for unreferenced retired or
abandoned unit sets.
