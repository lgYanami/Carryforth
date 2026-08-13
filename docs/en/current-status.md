# Carryforth Current Status and Capability Boundaries

> This document distinguishes code that exists, capabilities that can be evaluated locally,
> features that require explicit activation, work still under qualification, and surfaces not yet
> promised. Public source availability is not evidence of a packaged or production release.

## 1. Overall conclusion

Carryforth is an actively developed source project. Its current public scope is limited to:

- local builds and development from source;
- evaluation of core collaboration in a local single-Relay environment;
- development and verification of Desktop, Relay, ACP, `cf`, and project-domain capabilities;
- study of local-first, Nostr Relay, and Human–Agent project collaboration implementations.

The repository does not yet promise:

- stable binary installers;
- one command that installs every system dependency on a clean machine;
- production deployment or multi-instance high availability;
- a stable long-term upgrade path for existing data;
- a formal macOS or Windows support matrix;
- fully offline operation.

## 2. Current local evaluation scope

Local evaluation from source currently focuses on:

- Linux x86_64 Carryforth Desktop;
- a local single Relay and persistent dependencies;
- ACP Managed Agents and sidecars;
- a Linux x86_64 `cf` CLI;
- Channels, Messages, Project View, Documents, Project Context, and Meetings.

No Desktop package, standalone CLI archive, Relay OCI image, or other built artifact is included in
the current public scope. If artifact publication is considered later, its signing, installation,
automatic update, provenance, and upgrade experience will require a separate commitment and gate.

`web/` is currently a browser client in the source tree, not a release or support commitment.
Inherited Mobile, Harbor benchmark, Helm/Kubernetes, and legacy Hosted Push are outside the current
local source-evaluation scope.

## 3. Available foundation and explicit activation

### 3.1 Foundation established by startup

`./start.sh` can check local dependencies, prepare a private `.env`, start the development Compose
stack, run migrations, build Relay/CLI/Desktop, and connect Desktop to the local Relay.

It establishes a source-development runtime foundation. It does not make domain authorization
decisions on behalf of an owner, operator, or user.

### 3.2 Desktop preview switches

These surfaces are hidden by default and must be enabled in Settings → Experiments:

- Projects;
- Project View;
- Documents and Project Context, which follow the Project View preview surface;
- Meetings.

Enabling a preview switch only reveals the interface. Relay capability, durable Community gates,
initialization state, and member authorization remain independent requirements.

### 3.3 Community initialization

Starting processes does not automatically give a new Community the complete project model:

- Project View v3 requires operator preparation;
- the owner must review and sign the initialization command;
- Documents, Context, Meetings, and semantic capabilities each have readiness/enable contracts;
- a stable Relay signer is a prerequisite for several capabilities;
- a disabled or unprepared capability must fail closed instead of being fabricated by a client.

## 4. Capability status

### 4.1 Channels, Messages, and identity

Status: **core local capability**.

- Humans and agents use stable Nostr identities.
- Messages and Relay operations remain signed.
- Community membership and authorization remain in force.
- Desktop does not fall back to a legacy hosted service when the local Relay is unavailable.

This does not mean the system is anonymous, end-to-end encrypted, or compliance-certified.

### 4.2 Managed Agents and `cf`

Status: **core local capability; external runtimes and Providers require separate preparation**.

- `cf` provides signed, agent-facing Relay operations.
- ACP supports agent pools, sessions, and concurrent scheduling.
- Desktop can discover and manage Built-in, Goose, Claude Code, Codex, and other runtimes.

The Built-in Agent still requires a supported model Provider and credentials. External runtimes
require their corresponding CLI or adapter. Remote backend Providers are not part of the current
stable core product.

### 4.3 Project View and Role Continuity

Status: **implemented, preview by default, initialization required**.

- Supports Project Profile, Goal, Role, Plan, Stage, Requirement, Issue, Work, and Resource.
- Relay validates signatures, Revisions, and relationship contracts.
- Supports Role Proposals, Assignments, Work Commitments, Checkpoints, and Handoffs.

A new Community requires operator preparation and owner-signed initialization. `start.sh` does not
enable the database capability automatically.

### 4.4 Project Documents

Status: **implemented, preview by default, capability preparation required**.

- Stable document identity.
- Immutable full Revisions and a current Revision.
- History reads, pinned Revisions, and tombstones.
- Optimistic concurrency and draft protection on conflict.

A Document is a project record, not a secret store. It depends on a ready Project View v3 and a
stable signer.

### 4.5 Project Context

Status: **implemented, preview by default, asymmetric read/write surface**.

- Verified Edges/Hyperedges across Project View, Document, and Meeting.
- Exact, incident, and contains-all queries.
- Desktop graph canvas, inspector, and live updates.
- Edge maintenance through `cf` and agents.

Desktop is currently primarily a trusted read surface. Canonical attach/detach is primarily a CLI
operation. A Meeting may create a new Context binding only when its lifecycle and Action
Finalization conditions are satisfied.

### 4.6 Semantic graph-path queries

Status: **experimental, optional, gated, and not production-ready**.

Implemented:

- semantic Provider integration and a pgvector index;
- natural-language problem, optional initial coordinates, and query context;
- graph-root and path results;
- NIP-98 request binding, Relay-signed results, and canonical read commands;
- a controlled real-request path on a local single Relay.

Still under qualification:

- relevance and ranking quality under different Role/Work contexts;
- known-negative and relevance-floor calibration;
- PostgreSQL resource isolation, concurrency ladders, and long-running soak;
- production load balancers and multi-pod deployment;
- target scale, frozen SLOs, and complete recovery evidence.

This mechanism implements the design direction that Role/Work environments can shape returned
paths, but it does not guarantee that two environments produce different or semantically correct
results. Source startup enables the Worker/Query HTTP process switches by default, but it does not
thereby enable durable Community index/query gates. Semantic indexing can send project text
consisting of source type, the current visible title/name, and an optional summary to the
user-configured Provider; the current foundation does not send Document bodies or chunks. A query
sends its problem and relevant overview text to that Provider. The operator must enable the
corresponding Community gates separately; query activation additionally requires explicit
acknowledgement that the problem and overview text will cross that external Provider boundary.

### 4.7 Meetings

Status: **preview capability; multiple process switches and durable gates are off by default**.

V2 roster, Board, Floor, speech timeline, Handoff, moderator decisions, lease/timeout, close/abort,
and Action Finalization are implemented, including mixed Human and Agent participation.

Meeting creation, V2 direct actions, and Community read each have independent switches and approval
flows. Default visibility cannot be expanded by client declaration alone. The complete runtime
matrix remains under qualification.

### 4.8 Git Projects

Status: **preview Git collaboration workspace**.

It includes repository discovery, README/source browsing, branches, tags, commits and diffs,
Issues, PRs, inline comments, approvals, merges, conflict recovery, Git smart HTTP, and object
storage support.

It is not a mature GitHub replacement. Desktop hides it by default, and local operations require a
system `git` executable.

### 4.9 Media

Status: **locally evaluable, with format and resource boundaries**.

Relay provides content-addressed upload backed by MinIO. Desktop supports image, video, and general
file attachments, including drag/drop and paste, image processing, video transcoding, posters, and
Range streaming.

Limitations include:

- video and HEIC processing depend on system `ffmpeg`, which the root startup script neither
  installs nor checks;
- audio is currently rejected;
- PDFs are downloadable attachments without inline preview;
- there is no durable per-user total-storage quota;
- the Relay's per-file limit does not prove a safe Desktop memory bound for large-video handling.

## 5. Source publication and deferred artifact boundary

The current source publication does not include binaries, installers, containers, immutable release
tags, or other packaged artifacts. If Carryforth later publishes such artifacts, the strict
artifact gate currently fails closed on prerequisites including:

- third-party font and dependency licenses, SBOM, and vulnerability evidence;
- Relay runtime/container provenance;
- bundle identity and existing-data migration;
- owner-signed Project capability bootstrap;
- existing-data upgrade and canonical readback;
- clean-room E2E against published artifacts;
- private security reporting and release governance.

The current branch name, public source, a successful source build, or a local smoke test must not be
described as a packaged release, “production-ready,” “download and run,” or a “stable upgrade.” The
[artifact publication planning record](../stage/carryforth/open-source-release-surface-plan.md)
documents the deferred requirements; it is not the current publication plan.

## 6. Local-first limitations

The local Relay and data dependencies run in a user-controlled environment, and Desktop does not
use the legacy hosted control plane. These actions may still access the network:

- first-time Hermit, Cargo, pnpm, and container dependency downloads;
- user-configured model and semantic Providers;
- Git remotes;
- remote media, Resources, and external links.

The source-development Compose stack uses development credentials and publishes multiple ports on
host loopback. It is not a production-hardened deployment, and those bindings must not be widened
without a separate security design.

## 7. How to read “implemented”

In Carryforth documentation, “implemented” means that a corresponding protocol or component exists
and passed the test boundary recorded at that time. It does not automatically mean:

- enabled by default in a new environment;
- every Community has been migrated or initialized;
- every platform is formally supported;
- qualification at real scale and over long runtimes is complete;
- the surface is a stable public API;
- it can replace an external source of truth or understand the whole project automatically.

Before activation, verify current code, active operations documents, migration ledger, readiness,
and the corresponding qualification report together.
