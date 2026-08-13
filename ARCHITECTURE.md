# Carryforth Architecture

This document describes the architecture implemented by the current Carryforth
source tree. It is intentionally a high-level guide for contributors rather
than a frozen wire-schema, database-schema, or deployment specification.
Exact contracts remain in the domain crates, Relay handlers, migrations, and
tests linked below.

Carryforth is under active development. Its currently supported operating mode
is a **local source build with one Relay**. This document must not be read as a
production, high-availability, or stable-upgrade promise; see
[Current status](docs/en/current-status.md) and
[Local development](docs/en/local-development.md).

## 1. Design center

Carryforth begins with one product judgment:

> Continuity belongs to the project, not to any single agent.

A Human, Agent, model, Session, or Runtime may leave or be replaced. The
Project should continue to hold its identity, current state, responsibilities,
documents, relationships, collaboration outcomes, and provenance.

In the current implementation, one **Community** is the identity,
authorization, and tenant boundary of one **Project**. The two terms emphasize
different aspects of the same current boundary:

- Community describes membership, admission, and tenant isolation;
- Project describes the long-lived subject that owns project state.

This Project is not the same thing as a Git repository. One Project may refer
to several repositories, and the Desktop feature named Projects is an
experimental Git collaboration surface rather than the root Project identity.

The product model is described in [Core model](docs/en/core-model.md). The
architecture below explains how clients, Relay, storage, and optional Providers
enforce that model.

## 2. Supported topology and trust boundary

```text
 Human
   │
   ▼
 Carryforth Desktop ───────────────┐
                                  │ signed Nostr / authenticated HTTP
 Agent Runtime ⇄ ACP harness ─────┤
                                  │
 cf CLI ──────────────────────────┘
                                  ▼
                       Local Carryforth Relay
                    canonical project-state boundary
                          │       │       │
                 ┌────────┘       │       └──────────┐
                 ▼                ▼                  ▼
          PostgreSQL/pgvector   Redis          S3 / MinIO
          canonical + derived   coordination   media + Git objects

 Relay worker / query handler ── gated semantic egress ──► configured Provider

 Development auxiliaries: Keycloak, Prometheus, and Adminer
```

The current supported topology has one local Relay serving one local
development environment. PostgreSQL, Redis, and object storage are dependencies
of that Relay; clients do not maintain competing authoritative copies of
shared project state.

The design does **not** provide peer-to-peer project replication, Relay gossip,
or federated conflict resolution. Redis-backed fan-out and experimental mesh
code do not turn the Project into a peer-owned data set. Multi-Relay routing,
load balancers, and high availability are outside the currently supported
deployment surface.

Desktop also has no hosted fallback. If its configured local Relay is
unavailable, it reports a local connection failure instead of silently using a
legacy Buzz/Carryforth account, hosted Community, updater, or push service.
Explicit external operations—such as Provider calls, Git remotes, media URLs,
or dependency downloads—may still use the network.

## 3. Identity, tenancy, and authorization

### 3.1 Stable member identity

Humans and Agents use Nostr keypairs as stable member identities. A model,
Provider, Persona, process ID, ACP Session, and Runtime are not member
identities and do not own Project state.

WebSocket clients authenticate with NIP-42. HTTP operations that act as a
member normally use NIP-98 or another route-specific authenticated contract.
Optional owner attestation can associate a managed Agent with an eligible
owner, but it does not let a client invent membership or tenant scope.

Private keys are client credentials, not Project content. Desktop uses its
secret-storage boundary for identities; managed processes receive narrowly
defined environment variables. Provider API keys belong in the ignored local
`.env` or another controlled secret-injection mechanism. Keys must not be
stored in Project Documents, events, logs, fixtures, or screenshots.

### 3.2 Host-derived Community binding

Relay resolves the request's Community from the trusted request host/workspace
binding before a handler can observe tenant data. The resolved Community is
then carried through WebSocket, generic HTTP bridge, media, Git, search,
Meeting, and project-domain operations.

Client-controlled Nostr tags, payload fields, object IDs, or query coordinates
cannot select a different Community. A supplied project ID must agree with the
host-derived tenant, and unknown or inconsistent hosts fail closed. Database
queries and cache keys that can expose project data include the resolved
Community boundary.

### 3.3 Layered authorization

Authentication is only the first gate. A project-domain operation can also
depend on:

- Community membership, ban state, and base `owner`, `admin`, or `member`
  authority;
- channel membership for channel-scoped data;
- a domain capability and its durable Community enablement state;
- a current Role Assignment or other operation-specific authority;
- the current schema, object lifecycle, Revision, and generation;
- a stable configured Relay signer;
- exact request, signature, and canonical-body validation;
- Provider-egress acknowledgement and admission when external semantics are
  involved.

A Desktop preview toggle only reveals a UI. A running Worker or HTTP handler
only makes a process capable of serving work. Neither bypasses durable
Community gates or member authorization.

Relay advertises optional capabilities only when their required runtime and
Community readiness contracts hold. Callers must still handle a capability
closing or becoming stale between discovery and use.

## 4. Canonical state and derived state

Carryforth distinguishes authoritative project facts from aids used to read
them.

| Class | Examples | Authority |
|---|---|---|
| Canonical project state | Project View objects, Role Assignments and Checkpoints, Document Revisions and heads, Context bindings, Meeting records, signed Messages | Validated and committed inside the Community boundary |
| Relay projections | Relay-signed current object/head/catalog events | Verifiable read representation of canonical state |
| Derived state | Role Briefs, PostgreSQL full-text indexes, semantic embeddings, query rankings, UI caches | Rebuildable; cannot override canonical objects |
| External references | Git remotes, Resource targets, URLs, Provider responses | Not made authoritative merely by being referenced |

A member-signed command is a request to change state, not proof that the
change occurred. For the closed project domains, the normal pattern is:

```text
member-signed command
  └─► host binding + authentication + authorization
       └─► strict wire parser + pure domain reducer
            └─► Community-scoped atomic database commit
                 ├─► durable receipt / idempotency evidence
                 └─► stable Relay-signed projection(s)
```

Reads verify the expected schema, Relay signer, Community, object identity,
Revision, and lifecycle before treating a projection as current. Optimistic
concurrency fences reject a stale writer instead of silently rebasing or
overwriting its assumptions.

This separation is important for Agent use: a model summary, semantic result,
or local cache can guide the next read, but it cannot silently become the
Project's new truth.

## 5. Project-domain architecture

### 5.1 Project View v3

Project View is the first-order current state of the Project. Its stable
objects are:

- Project Profile;
- Goal;
- Role;
- Plan;
- Stage;
- Requirement;
- Issue;
- Work;
- Resource.

The pure contracts and reducers live in
[`buzz-project-view`](crates/buzz-project-view/src/lib.rs). Relay supplies
authentication, readiness, signing, and delivery; `buzz-db` owns the
Community-scoped transaction and current-state checks.

Project View is a constrained domain model, not arbitrary JSON storage. It
validates object-local Revisions, relationships, lifecycle changes, and
cross-object references. A Resource is a stable project entry point whose
Guide is an ordinary Project Document; registering a Resource does not fetch,
install, execute, or authorize its external target.

### 5.2 Role Continuity

Role Continuity is part of the Project View v3 domain but separates several
identities that are often collapsed in an Agent system:

```text
Role                stable responsibility held by the Project
  └─ Assignment     bounded tenure held by one Member
       └─ Member    stable Human or Agent public-key identity
            └─ Runtime   replaceable execution process/session

Work responsibility persists across tenures
Work Commitment belongs to one exact Assignment
Checkpoint and Handoff externalize the situation
Role Brief is derived from canonical state
```

Role Proposals and governance checks control how an Assignment becomes active.
Work Responsibility belongs to the stable Role; a Work Commitment records the
current assignee's explicit acceptance during one tenure. Ending an Assignment
does not complete or delete its Work, and a successor creates a new Assignment
and Commitment instead of inheriting the predecessor's identity.

Checkpoints are append-oriented records of progress, blockers, risks, and next
steps. A Handoff may add targeted transition information, but continuity does
not depend on the predecessor remaining online long enough to produce an exit
summary. The current Role Brief is a derived, attributable reading surface over
Project state, not another writable memory store.

See [Core design: Role Continuity](docs/en/core-design/role-continuity.md).

### 5.3 Project Documents

A Project Document has a stable identity, immutable full Revisions, and an
explicit current head. Saves use expected-current Revision checks; deletion is
represented by a verifiable tombstone rather than title reuse or an ambiguous
missing row.

The pure command, reducer, and projection contracts live in
[`buzz-project-document`](crates/buzz-project-document/src/lib.rs). Member
commands and Relay projections are separate wire roles, and the database
transaction advances the Document and catalog state together.

Documents carry designs, explanations, constraints, Guides, Meeting outcomes,
and Context meaning. They are not a secret store and do not automatically
execute Markdown, resolve Resources, or grant access to referenced systems.

### 5.4 Project Context

Project Context preserves second-order semantics: why exact project objects
must be understood together. It uses one undirected Edge/Hyperedge with two
separate responsibilities:

```text
Project Context Edge
├─ exact set of two or more stable Coordinates
└─ one or more versioned Context Documents containing the explanation
```

Coordinates currently refer to Project View objects, Project Documents, or
Meetings. The Edge fixes the scope of a relationship; its Documents carry the
reason, dependency, impact, exception, or applicability boundary. The model
does not require an expanding global enum of relationship meanings, and it
does not infer or create Edges from similar text.

The pure contracts are in
[`buzz-project-context`](crates/buzz-project-context/src/lib.rs). Relay checks
that Coordinates are current and in the same Community before committing a
binding. Tombstoning a Coordinate does not silently rewrite the historical
identity of an existing relationship.

Desktop provides graph navigation and inspection. Canonical relationship
maintenance uses validated project commands, principally through `cf` and
Agent-facing operations.

See [Core design: Coordinates before context](docs/en/core-design/coordinate-and-context.md).

### 5.5 Meetings

A Meeting is a bounded Project-level deliberation, not a bag of chat messages.
The current model combines:

- a fixed participant roster and moderator;
- an agenda and shared Board;
- controlled Floor, canonical Speech, and Directed Handoff;
- leases, timeout/recovery rules, and explicit terminal states;
- Action Finalization for recording agreed follow-up through ordinary domains.

Humans and Agents contribute from different Roles, Work, and previously read
Project context. The moderator-maintained Board is the explicit convergence
point for the discussion, but it is not automatically a Project decision or a
Project View mutation.

Meeting outcomes become long-lived Project state only when an authorized
member uses the normal Work, Document, Context, Checkpoint, or other domain
command and reads back the committed result. Action Finalization coordinates
those writes; it is not a privileged materializer that bypasses their normal
authorization and Revision checks.

Meeting remains gated and under qualification. Creation, Community read, and
direct-action capabilities have separate readiness and authorization
conditions. See [Core design: Meeting](docs/en/core-design/meeting.md).

### 5.6 Channels, Messages, media, and Git

Channels and signed Nostr Messages provide everyday collaboration. Channel
visibility and membership are distinct from Project-wide domain authority.
Private or recipient-scoped material is filtered at historical read and live
fan-out boundaries.

Media uses content-addressed Blossom-style operations backed by S3-compatible
storage. Git collaboration uses NIP-34-style records, smart HTTP, and
content-addressed object-store packs/manifests. These are useful Project
surfaces, but neither a media object nor a Git repository becomes the Project
identity.

## 6. Context-aware semantic graph retrieval

Semantic retrieval is an optional, derived read path over the one Project-owned
Context graph. It is not an Agent-private memory system and does not create,
delete, or rewrite Project relationships.

### 6.1 Derived semantic index

```text
current canonical Project View / Document / Meeting source
  └─► typed source observation + currentness digest
       └─► deterministic bounded overview
            └─► gated background job
                 └─► configured external embedding Provider
                      └─► immutable generation + current semantic head
```

The index stores embeddings and provenance for current canonical sources in
PostgreSQL/pgvector. An embedding is accepted only against the expected source
snapshot and model contract. A changed, deleted, unsupported, or superseded
source cannot be presented as the current head merely because an older vector
still exists.

The external Provider receives bounded encoder input, not an unrestricted dump
of Document bodies, Meeting transcripts, or the full graph. Provider egress is
still real data egress and must be explicitly configured and authorized.

### 6.2 Query path

A query contains a required natural-language problem and may include:

- explicit initial Coordinates, which specify structural starting points;
- context Coordinates such as Role or Work, which provide soft query-time
  relevance lenses.

The problem remains the primary signal. Context Coordinates can change recall
and ranking, but they are not ACLs, mandatory roots, or private subgraphs. The
query follows only real current undirected Hyperedges; semantic similarity is
used to choose where to read, not to invent a relationship.

```text
authenticated, request-bound query
  └─► process + Community gate and current-generation checks
       └─► bounded Provider encoding of problem / conditioned inputs
            └─► current semantic recall
                 └─► bounded traversal of a current Context snapshot
                      └─► budgeted response packing
                           └─► Relay-signed result + canonical read targets
```

The result carries request binding, caller/Project identity, source and graph
currentness evidence, paths, and Relay signature material. The `cf` CLI
verifies that result and derives normalized read commands that return to the
canonical source domains. A result is navigation and provenance, not a copied
or newly canonical context body.

Semantic operation requires more than Provider credentials. The Worker and
query-handler process switches, durable Community index/query gates, active
generation, Provider-egress acknowledgement, stable signer, routing policy,
and resource admission must agree. A disabled or stale condition fails closed.
Closing semantic capability does not delete Project View, Documents, Context,
Meetings, or other canonical Project state.

This capability remains experimental and is not production-qualified. See
[Core design: Context-aware semantic graph retrieval](docs/en/core-design/context-aware-semantic-graph-retrieval.md)
and [Semantic operations](docs/semantic-pgvector-operations.md).

## 7. Relay processing model

Relay is both the protocol boundary and the coordinator of domain services. It
does not delegate tenant selection or canonical-currentness decisions to a
client.

### 7.1 Generic Nostr flow

For ordinary Nostr events, Relay:

1. binds the request to a Community;
2. authenticates the connection or HTTP request;
3. verifies the event identity, signature, kind, and applicable scope;
4. applies Community/channel visibility and operation-specific checks;
5. persists accepted state in PostgreSQL;
6. publishes eligible live updates locally and through Redis coordination;
7. schedules applicable audit, workflow, or domain side effects.

Ephemeral protocol data and disabled audit paths deliberately have different
persistence rules. The audit hash chain is tamper-evident evidence for the
operations it records; it is not a complete record of every external action, a
tamper-resistant ledger against a database administrator, or a compliance
certification.

### 7.2 Closed project-domain flow

Project View, Documents, and Context add stricter adapters around the generic
transport:

- pure domain crates own validation and deterministic reduction;
- `buzz-sdk` owns exact command/projection construction and parsing;
- `buzz-db` owns Community-scoped locks, current-state reads, receipts, and
  atomic persistence;
- `buzz-relay` owns credentials, capability gates, stable signing, error
  mapping, and post-commit delivery;
- Desktop and `cf` verify and present the resulting read contracts.

This split lets reducers be tested without networking or a database while
keeping authority and currentness checks at the actual commit boundary.

### 7.3 Read flow

Nostr subscriptions and generic queries must use explicit kinds. Before a
result is returned, Relay applies tenant, member, channel/recipient,
projection-schema, signer, and lifecycle filters appropriate to that data.

PostgreSQL full-text search is a derived index over stored searchable content.
The Relay re-authorizes candidates before delivery; a search hit alone is not
read permission.

### 7.4 HTTP surfaces

Nostr events are preferred for state that fits the event model. HTTP remains
for surfaces that genuinely require it, including:

- the generic event/query/count bridge;
- NIP metadata and health/readiness probes;
- semantic graph queries;
- Blossom media transfer;
- Git smart HTTP and policy callbacks;
- invites, local/operator/runtime administration, and optional admin reads;
- workflow webhooks and audio WebSocket upgrade.

Each route keeps its own authentication and body/admission limits. The list is
architectural, not a stable public API promise; consult the router and client
contracts before extending it.

## 8. Clients and Agent runtimes

### 8.1 Carryforth Desktop

Desktop is a Tauri 2 application with a React frontend. It is the primary Human
interface for Messages, Agents, Project View, Documents, Context, Meetings,
media, and preview Git collaboration.

The Tauri layer owns native concerns such as process lifecycle, sidecars,
filesystem integration, and supported secret storage. The React layer owns
feature presentation and client-side query state. Client caches are scoped to
the active Community and must be reset when Community switching remounts the UI.

Desktop can hide experimental surfaces, but it cannot manufacture Relay
capability or bypass domain authorization. A visually current page is not
authoritative unless the underlying response passed its signature, Revision,
and readiness checks.

### 8.2 `cf` and `buzz-sdk`

`cf` is the Agent-first CLI. It provides typed operations for Messages,
Channels, Project View, Documents, Context, Meetings, Roles, Resources, media,
and related collaboration surfaces. It signs requests, verifies closed
responses, and keeps canonical readback explicit.

`buzz-sdk` contains reusable wire builders, strict parsers, authentication
helpers, and typed client utilities. It does not replace Relay authorization.

### 8.3 ACP-managed Agents

`buzz-acp` bridges Relay events to an ACP-speaking child process over stdio
JSON-RPC. `buzz-agent`, `buzz-dev-mcp`, and the `sprig` multicall bundle provide
the built-in Agent and tool surfaces used in development and packaging.

The harness controls credentials and Runtime/session lifecycle. The model-facing
Agent acts through bounded tools and project commands; the Runtime is not given
ownership of Project data simply because it is running. Runtime replacement is
handled by Role Continuity rather than by copying a private authoritative
memory store.

### 8.4 Other browser surfaces

`web/` and `admin-web/` remain source-tree surfaces, not a committed public
deployment or support boundary. The optional admin router is host-gated and
configuration-gated. Neither surface is a hosted fallback for Desktop.

## 9. Persistence and supporting services

| Component | Current responsibility | Boundary |
|---|---|---|
| PostgreSQL / pgvector | Nostr events, Community/member and channel state, project-domain canonical tables and receipts, Meeting/workflow state, full-text search data, semantic jobs/generations/vectors | Primary durable state for the local Relay |
| Redis | Pub/sub delivery coordination, replay protection, admission/rate coordination, presence/typing and selected runtime caches | Coordination, not canonical Project ownership |
| S3-compatible storage / MinIO | Content-addressed media and Git pack/manifest objects | Blob/object data; references remain authorized by Relay |
| OS secret storage or restricted local fallback | Desktop member and managed-Agent private keys | Client-local credential boundary |
| Keycloak | Local OAuth/OIDC development infrastructure | Auxiliary; not the canonical Nostr Project-member identity |
| Prometheus | Local metrics collection | Operational telemetry, not project facts |
| Adminer | Local database inspection | Development-only operator tool |

The schema has two maintained views:

- [`migrations/`](migrations/) is the forward installation/upgrade path;
- [`schema/schema.sql`](schema/schema.sql) is the aligned fresh-schema
  snapshot used by checks and inspection.

A database change is incomplete unless migrations, the schema snapshot,
readiness checks, and relevant drift/upgrade tests agree. Applied migrations
must not be edited in place to make a local database appear current.

## 10. Source layout

The principal code boundaries are:

| Path | Responsibility |
|---|---|
| [`crates/buzz-core`](crates/buzz-core/) | shared Nostr kinds, verification, tenant and protocol types |
| [`crates/buzz-auth`](crates/buzz-auth/) | NIP-42/NIP-98 authentication, scopes, and auth contracts |
| [`crates/buzz-project-view`](crates/buzz-project-view/) | pure Project View v3 and Role Continuity domain contracts |
| [`crates/buzz-project-document`](crates/buzz-project-document/) | pure versioned Document contracts and reducer |
| [`crates/buzz-project-context`](crates/buzz-project-context/) | pure Coordinate/Hyperedge contracts and reducer |
| [`crates/buzz-semantic`](crates/buzz-semantic/) | semantic-source and deterministic overview contracts |
| [`crates/buzz-semantic-query`](crates/buzz-semantic-query/) | bounded query, ranking, traversal, and result contracts |
| [`crates/buzz-db`](crates/buzz-db/) | PostgreSQL persistence and atomic domain coordinators |
| [`crates/buzz-relay`](crates/buzz-relay/) | network boundary, tenant/auth gates, signing, workers, and delivery |
| [`crates/carryforth-cli`](crates/carryforth-cli/) | Agent-first `cf` CLI |
| [`crates/buzz-sdk`](crates/buzz-sdk/) | typed event/request builders and strict wire parsers |
| [`crates/buzz-acp`](crates/buzz-acp/) | ACP harness and managed-Agent scheduling |
| [`desktop/`](desktop/) | Tauri/React Desktop application |
| [`web/`](web/) and [`admin-web/`](admin-web/) | source-only browser surfaces |
| [`migrations/`](migrations/) and [`schema/`](schema/) | upgrade path and fresh schema |
| [`scripts/`](scripts/) and [`Justfile`](Justfile) | supported development, validation, and lifecycle tasks |

Several inherited crate, binary, environment, database, bundle, and keyring
identifiers retain `buzz-*` or `BUZZ_*`. They are compatibility coordinates,
not current product identity. Do not mechanically rename them; see
[UPSTREAM.md](UPSTREAM.md).

## 11. Local source-development boundary

The supported entry point is:

```bash
./start.sh
```

It checks external prerequisites, activates the repository-pinned toolchain,
prepares a private `.env`, starts the development Compose dependencies, applies
migrations, and builds/starts Relay and Desktop. It does not install Docker,
Python, or operating-system packages, and it preserves existing local volumes
unless the user explicitly chooses a destructive reset.

The source-development stack uses development credentials and publishes
multiple ports to host loopback. The checked-in `.env.example`, raw Relay
default, and checked-in Compose file all keep those listeners on loopback, but
development HTTP-token and Relay-membership gates are not production hardening.
Run it only on a trusted machine, review `.env`, and do not deliberately widen
the bindings to an untrusted LAN or the Internet.

Local-first means that Project state is controlled by the user's local Relay;
it does not mean fully offline or automatically hardened. First-time toolchain,
Cargo, pnpm, and container downloads, external model/semantic Providers, Git
remotes, and remote resources can all leave the machine.

## 12. Contributor invariants

Changes must preserve these architectural boundaries:

1. Derive the Community from trusted host/workspace state, never from a
   client-controlled tag or payload.
2. Keep member commands, Relay-signed projections, and derived read models
   distinct.
3. Recheck authority and currentness at the database commit boundary; do not
   trust a prior UI read or semantic hit.
4. Prefer signed Nostr events over endpoint-specific HTTP when the event model
   is sufficient.
5. Define event kinds centrally and require explicit kinds in Relay filters.
6. Keep pure reducers free of database, network, signer, and runtime concerns.
7. Treat semantic vectors, full-text indexes, Role Briefs, and caches as
   rebuildable derived state.
8. Never let a Provider response, Agent summary, Meeting Board, or external
   Resource silently mutate canonical Project relationships.
9. Keep migrations, schema snapshots, readiness, and drift tests aligned.
10. Do not expose private keys, Provider credentials, private infrastructure,
    or user data in source, documentation, logs, or fixtures.

Repository-specific implementation and validation rules are in
[AGENTS.md](AGENTS.md), [CONTRIBUTING.md](CONTRIBUTING.md),
[TESTING.md](TESTING.md), and [SECURITY.md](SECURITY.md).
