# Carryforth System Overview

> This document introduces Carryforth's components, data flow, identities,
> authorization, and network boundaries from the perspective of a running
> product. For protocol- and crate-level details, see
> [ARCHITECTURE.md](../../ARCHITECTURE.md).

## 1. Overall structure

```text
┌──────────────────────────────────────────────────────────────────┐
│ Carryforth Desktop     Managed Agents / ACP       cf CLI         │
└───────────────┬──────────────────┬──────────────────┬────────────┘
                └──────────────────┼──────────────────┘
                                   │ signed Nostr events
                                   ▼
                         Local Carryforth Relay
                         (internal crate: buzz-relay)
                                   │
                 ┌─────────────────┼─────────────────┐
                 ▼                 ▼                 ▼
             PostgreSQL           Redis           S3 / MinIO
```

The current supported topology is a single local Relay. Desktop, Managed
Agents, and `cf` do not each maintain competing copies of project facts. They
read and submit Community-scoped state through Relay.

The Project / Community discussed here is not the same concept as the Desktop
feature named **Projects**:

- Project / Community is Carryforth's boundary for long-lived collaboration,
  identity, and data;
- Projects is a collaboration preview surface for Git repositories, branches,
  commits, Issues, and PRs.

One Carryforth Project can be associated with multiple code repositories. The
system does not equate a "project" with "one Git repository."

## 2. Main components

### 2.1 Carryforth Desktop

Desktop is currently the primary Human interface. It is built with Tauri 2 and
React 19 and provides interfaces for Channels, Messages, Agent management,
Project View, Documents, Project Context, Meetings, Git Projects, media, and
more.

Some advanced Desktop features remain controlled by preview toggles under
Settings → Experiments. A client toggle only controls whether an interface is
shown; it does not bypass Relay capabilities, Community gates, or authorization
checks.

### 2.2 Local Relay

Relay is the canonical state and authorization boundary. It is responsible for:

- receiving, filtering, signing, and querying Nostr / NIP events;
- binding hosts to Community tenant boundaries;
- checking membership, authorization, and capabilities;
- validating and persisting Project View, Documents, Context, and Meetings;
- the HTTP bridge, NIP metadata, media, Git smart HTTP, and other surfaces that
  genuinely require HTTP;
- background workers, auditing, and runtime readiness.

An event tag supplied by a client cannot choose the tenant. Relay must resolve
the Community from trusted host / workspace coordinates so a client cannot
write data into another project boundary.

### 2.3 The `cf` CLI

`cf` is Carryforth's Agent-facing CLI. It signs Relay requests and provides
closed read/write contracts for Messages, Channels, Project View, Documents,
Project Context, Meetings, Git / PR, media, Roles, Resources, and workflows.

Managed Agents receive a controlled session through `CARRYFORTH_RELAY_URL`,
`CARRYFORTH_PRIVATE_KEY`, and `CARRYFORTH_AUTH_TAG`. Private keys must never
appear in logs, documentation, or command examples.

### 2.4 ACP and Managed Agents

`buzz-acp` bridges Relay sessions to ACP stdio JSON-RPC. Desktop can discover
and manage Built-in, Goose, Claude Code, Codex, and other Runtimes and track
their startup, shutdown, logs, and session state.

A Runtime is a replaceable execution instance, not the owner of Project state.
The Built-in Agent is not an embedded offline model; it still requires
Anthropic, OpenAI-compatible, Databricks, or another supported Provider to be
configured. External Runtimes require the corresponding CLI or adapter.

### 2.5 Data and dependencies

- PostgreSQL / pgvector: canonical projections, project domain state, and the
  optional semantic index;
- Redis: runtime coordination, queues, or cache-like dependencies;
- MinIO: content-addressed media objects;
- Keycloak: development identity infrastructure;
- Prometheus: local runtime metrics.

The specific ownership of data is defined by the schema, migrations, and the
corresponding domain code. This overview must not be used to infer table-level
contracts.

## 3. Canonical and derived state

Carryforth distinguishes canonical project state from derived read results:

- Project View objects, Document Revisions, Context Edges, Meeting state, and
  signed messages are canonical records;
- Role Briefs, client- or model-generated summaries, indexes, query results,
  and some UI caches are read surfaces derived from canonical state;
- derived state cannot overwrite authoritative Relay objects when they
  conflict;
- semantic query results are not written back to canonical history as new
  virtual Events.

This distinction prevents an Agent summary, one query, or a client cache from
silently becoming a new source of project truth.

## 4. Identity and authorization

### 4.1 Stable identity

Humans and Agents use Nostr keypairs as stable identities, and Relay operations
remain signed. Agent Runtime, model, Provider, Persona, and process ID are not
the member identity itself.

Desktop prefers to store private keys in the operating system keyring. When no
usable keyring is available, supported paths may fall back to a local file with
restricted permissions. Provider API Keys are stored instead in a private
`.env` ignored by Git. They form a credential boundary distinct from Nostr
member identities and Relay signing keys.

### 4.2 Community boundary

Currently, one Community is the tenant boundary of one Project. Project View,
Documents, Context, Meetings, Messages, and membership state must all belong to
the same Community. Cross-Project references and writes should fail closed.

### 4.3 Authorization layers

Authorization is not one frontend boolean. Several layers must hold together:

- host / workspace binding;
- Community membership and ban state;
- base `owner`, `admin`, or `member` level;
- domain capability and durable gate;
- current object Revision, generation, or lifecycle;
- caller signature and request-body binding;
- Provider egress checks and revalidation before releasing results.

An interface being visible or a process being started therefore does not mean
the current member automatically has permission to write or make an external
query.

## 5. What local-first means

Desktop connects by default to:

```text
ws://localhost:3000
```

If the local Relay is unavailable, Desktop reports a local connection error. It
does not fall back to an old hosted Carryforth / Buzz account, Community,
updater, or push service.

"Local-first" does not mean:

- all network requests are prohibited;
- all development ports bind only to loopback;
- the system needs no identity or authorization;
- Providers, remote media, or Git remotes never access the network;
- development Compose has production-grade hardening.

The source development environment publishes several service ports to the host
and uses development configuration and credentials. Do not expose it directly
to an untrusted network.

## 6. Semantic Provider boundary

Project Context semantic graph queries use a user-configured external Provider
to generate vectors. The contract limits which indexed information may enter a
Provider request, and a query must also pass the Community gate, member
authorization, object currentness, Provider admission, result signing, and
other checks.

When queries are enabled, the operator must explicitly acknowledge that the
problem / overview will leave the local control plane and be sent to the
configured Provider. A Provider API Key should exist only in a private local
environment or controlled secret injection. It must not be written to a Project
Document, log, or event.

Semantic query is an optional capability. Stopping the Worker and Query HTTP
processes, or closing the Community query gate, does not delete Project View,
Documents, Context Edges, or other canonical project data.

## 7. Media and external content

Relay provides Blossom-style content-addressed uploads and stores media in
MinIO. Desktop supports image, video, and ordinary file attachments. Local
image processing and video transcoding may depend on the system `ffmpeg`, which
the root startup script neither installs nor checks.

External links, Git remotes, media URLs, and Resource Guides can all point
outside the local machine. Recording an object in a local Project does not mean
its referenced external content has been copied offline, will remain available,
or is controlled by Carryforth authorization.

## 8. Audit and security boundaries

Audit records and hash chains help detect inconsistency and tampering, but they
must not be described as:

- having obtained a compliance certification;
- impossible to recompute by an attacker with write access to the underlying
  database;
- able to audit every action performed by external systems;
- providing end-to-end encryption for user data.

See [SECURITY.md](../../SECURITY.md) for the security model and known
boundaries. Security vulnerabilities must be reported privately as described
there.

## 9. Compatibility identifiers

The codebase still retains many `buzz-*` crate / binary names, `BUZZ_*`
environment variables, database identifiers, Nostr kinds, and bundle / keyring
/ app-data coordinates. These names may be wire, storage, or existing-data
continuity contracts.

They are not the current product identity, and they must not be mechanically
renamed merely to make wording uniform. Any change requires a separate data and
protocol migration design. See [UPSTREAM.md](../../UPSTREAM.md) for provenance
and attribution.
