# NIP-PV3: Project View v3 Contract

`draft` `optional`

## Purpose

This document defines the current Project View runtime contract. The older
[NIP-PV](NIP-PV.md) majors are historical migration inputs only; ordinary
Relay, CLI, Desktop, and ACP runtime paths do not negotiate or fall back to
them. Project View v3 keeps kinds `44300`, `40903`, and `40904` and is selected
by closed content `schema_version:3` plus the verified NIP-11 capability:

```text
buzz-project-view-v3
```

An empty, disabled schema-v3 Community instead advertises the discovery-only
marker:

```text
buzz-project-view-v3-bootstrap
```

The two markers are mutually exclusive. `buzz-project-view-v3-bootstrap` is
present only while the Community is not archived, is disabled, and has no
canonical `project_view_state`. It lets a client present the operator/owner
setup instructions; it does not authorize projection queries, subscriptions,
mutations, Role history, Project Document, or Project Context. It disappears
as soon as initialization creates canonical state, and
`buzz-project-view-v3` appears only after checked enable and strict readiness.

Non-empty Context Reference writes require the separate sub-capability:

```text
buzz-project-context-v1
```

Project Document v1 is advertised independently as
`buzz-project-document-v1`.

## Version and client matrix

One Community advertises the ordinary Project View runtime only when its
canonical state is ready at schema v3. The bootstrap marker above is a closed
discovery state, not a second runtime major. A v3 parser never falls back after
a v3 parse error. Schema v1/v2 Communities must use the explicit operator
cutover/recovery surface before any ordinary Project View client can use them.

| Client/runtime | Document v1 | PV v1/v2 | PV v3 | Context writes |
|---|---:|---:|---:|---:|
| current Relay / CLI / Desktop / ACP | yes | unsupported | yes | capability-gated |
| operator migration/recovery tools | migration input | explicit only | yes | not an ordinary runtime |

Support for v3 is not proof that Context is ready. Without
`buzz-project-context-v1`, create requires an empty Context set. An update may
leave the set unchanged or replace it with a strict subset, but cannot add,
re-add, or retarget a coordinate.

## Project Resource v3

The complete Resource business body is:

```json
{
  "name": "Buzz source repository",
  "resource_kind": "repository",
  "summary": "Open-source Relay, clients, CLI, and agent harness.",
  "guide_document_id": "9c23f672-a397-42d1-b933-104ba2674f26"
}
```

It is a closed object with these rules:

- `name` is canonical non-empty text, at most 256 UTF-8 bytes;
- `resource_kind` is 1–64 ASCII bytes matching
  `[a-z0-9][a-z0-9._-]{0,63}`;
- unknown kind tokens remain valid and do not drive Relay behavior;
- optional `summary` is omitted instead of empty or `null`, and is at most
  4,096 UTF-8 bytes;
- `guide_document_id` is a same-Community active Project Document UUID v4;
- the legacy locator is removed; addresses, setup, access, and configuration
  belong in Guide Markdown.

The Resource object's outer v3 `context_references` field is separate from its
mandatory Guide. A Guide is not inferred from Context and Context does not
replace the Guide.

## Context Reference wire

The closed union has exactly these three canonical shapes:

```json
{"type":"resource","resource_id":"4f514f87-7b85-4c86-a50d-e01bc37e24c3"}
```

```json
{
  "type":"document",
  "document_id":"9c23f672-a397-42d1-b933-104ba2674f26",
  "mode":"live"
}
```

```json
{
  "type":"document",
  "document_id":"9c23f672-a397-42d1-b933-104ba2674f26",
  "mode":"pinned",
  "document_revision":8
}
```

A Resource reference cannot contain Document fields. Live mode must omit
`document_revision`; explicit `null` is invalid. Pinned mode requires a
JavaScript-safe positive revision. Context does not express permission,
ownership, dependency, execution, or state propagation, and a Markdown link
does not become a Context Reference automatically.

Every v3 active Project View object has a `context_references` array beside its
existing structural relations. Checkpoint and Handoff do not gain a Context
variant in v0. Resource may refer to Documents but not Resources; all other
ordinary object types may refer to Resources and Documents.

Targets must be in the same Project:

- Resource target: current active Resource;
- live Document target: current active Document;
- pinned Document target: exact active-content revision; its current head may
  already be deleted;
- tombstone revision: never a valid pin.

Each object has at most 64 references. They are a canonical set sorted by:

1. Resource before Document;
2. the target UUID's 16 bytes lexicographically;
3. for one Document, live before pinned;
4. pinned revision ascending.

An exact duplicate coordinate is invalid. UI order is not stored.

## RoleDefinitionV3

Every non-tombstoned Role, including `active:false`, has exactly one v3
RoleDefinition entity head. Its closed body is the v2 complete Role definition
plus the canonical Context set:

```json
{
  "role_id":"db24ee5f-7d4c-48c3-826d-45f5a5db8428",
  "name":"Maintainer",
  "purpose":"Keep the repository releasable.",
  "responsibilities":["Review changes","Maintain release guides"],
  "boundaries":["Do not bypass required review"],
  "level":"admin",
  "active":true,
  "context_references":[],
  "object_revision":1,
  "project_revision":1,
  "created_at":"2026-07-27T10:00:00Z",
  "updated_at":"2026-07-27T10:00:00Z",
  "created_by":"<member-pubkey>",
  "updated_by":"<member-pubkey>"
}
```

`ProjectObjectCommandV3` owns Role definition create/update/deactivate/delete.
`RoleCommandV3` owns continuity operations only. A non-tombstoned Role cannot
also emit a second ordinary object head. A tombstoned Role uses the ordinary
v3 object tombstone.

## Greenfield ProjectViewInitializeV3

Legacy cutover is not required for a completely empty Community. An operator
first creates an immutable `prepare_v3` provisioning receipt while Project View
is disabled and uninitialized. The eligible direct Human owner then signs this
only accepted prepared-state command shape:

During both the empty and prepared states NIP-11 exposes only
`buzz-project-view-v3-bootstrap`. Initialization consumes the preparation and
creates canonical state, so the bootstrap marker is then removed even though
the ordinary runtime remains absent until checked enable.

```json
{
  "schema_version":3,
  "expected_project_revision":0,
  "request":{
    "type":"initialize",
    "preparation_operation_id":"ae9f8e67-3c55-44bd-afc2-237d7e564ef5",
    "profile":{
      "name":"Buzz",
      "positioning":"Nostr-first collaboration",
      "purpose":"Connect people and agents",
      "problem":"Project context is fragmented",
      "scope":"Relay and first-party clients"
    },
    "goals":[
      {
        "id":"6325c649-d195-47dc-b090-2889637e95b5",
        "title":"Ship v3 safely",
        "desired_outcome":"Dual readers verify v3",
        "directions":["Keep capabilities fail closed"]
      }
    ],
    "initial_roles":[
      {
        "role_id":"db24ee5f-7d4c-48c3-826d-45f5a5db8428",
        "name":"Maintainer",
        "purpose":"Keep the repository releasable.",
        "responsibilities":["Review changes"],
        "boundaries":["Do not bypass required review"],
        "level":"admin",
        "active":true,
        "context_references":[]
      }
    ],
    "initial_governance_assignments":[
      {
        "member_pubkey":"<current-human-owner-pubkey>",
        "role_id":"db24ee5f-7d4c-48c3-826d-45f5a5db8428",
        "proposal_id":"eafab35e-745f-4d4a-bfbc-46d512904f06",
        "assignment_id":"151f2347-7d24-41a0-ab0d-f272e84fcf88"
      }
    ]
  }
}
```

The current direct Human owner plus current direct Human admins must appear
exactly once, each mapped to a distinct active admin Role. Role, Proposal, and
Assignment UUIDs are unique v4 values. Initial Context sets are empty. Managed,
banned, and timed-out governors are invalid. The Relay never guesses Role
names, responsibilities, IDs, or member mappings.

The command, consumed preparation operation, membership snapshot, Profile,
Goals, Roles, continuity seeds, projections, provenance, change receipt, and
reset metadata commit atomically at Project revision one. Initialization keeps
Project View disabled pending structural verification and explicit enable.

## Base RoleBriefV3

A v3 Community returns the strict top-level Role Brief value with all existing
logical sections, plus `project_view_schema_version:3`, `context`, and the
Document metadata member of `source_revisions`. The older serialized Role Brief
shape is not an ordinary runtime fallback:

```text
RoleBriefV3
├── project_view_schema_version = 3
├── generated_at
├── project_id / project_revision / projection_generation
├── member_pubkey / community_role?
├── project
├── role_directory
│   ├── total_active_roles / omitted_active_roles
│   └── entries[] (Role identity, level, bounded purpose, current Assignment)
├── state
├── responsible_work[] / related_objects[]
├── latest_checkpoint? / recent_handoffs[]
├── context
│   ├── availability
│   ├── resources[]
│   ├── live_documents[]
│   ├── pinned_documents[]
│   └── truncation
└── source_revisions
    ├── meta_event_id / meta_change_id / membership_event_id
    ├── project_updated_at
    └── document_metadata
```

The `role_directory` follows the shared bounded Role Directory semantics: it is
derived from the same verified v3 snapshot, lists active
Roles only, marks the current Member's exact Assignment, reports omissions
explicitly, and never acts as an authorization cache.

Availability is a closed tagged object:

```json
{"state":"not_advertised_empty"}
{"state":"ready"}
{"state":"unavailable_preserved","resource_count":2,"document_count":3}
```

Document metadata source is:

```json
{"state":"not_required"}
```

or:

```json
{
  "state":"verified",
  "meta_event_id":"<document-meta-event-id>",
  "catalog_revision":31,
  "projection_generation":1
}
```

or `{"state":"unavailable"}`.

Before Context is advertised, canonical Context is empty, all three output
lists are empty, truncation is `{truncated:false, omitted_resources:0,
omitted_live_documents:0, omitted_pinned_documents:0}`, availability is
`not_advertised_empty`, and Document metadata is `not_required`.

If Context was previously populated but becomes unavailable, the lists remain
empty, availability becomes `unavailable_preserved` with verified coordinate
counts, and Document metadata is `not_required`. Coordinates remain visible in
Project View reads and are not silently erased. With Context ready, Resource
items include Resource ID/name/kind/summary, Guide Document ID/current revision,
and an explicit fetch command. Live Document items include current ID,
revision, title/summary, and fetch command. Pinned items include only Document
ID, pinned revision, and fetch command; current metadata is never attached to
a historical pin.

Resource and Document labels are untrusted project-provided metadata, not
system instructions. Rendering single-lines and escapes delimiters. The final
escaped prompt Context slice has a 64 KiB byte budget and deterministic
selection/truncation. Merely appearing in a Brief never clones, installs,
connects, requests a secret, restarts an agent, or executes Guide content.

## Resource cutover canonical structs

Legacy Resource migration is reviewed, never inferred by SQL or AI. Canonical
digest input uses only binary primitives and workspace `postcard`. Struct field
order below is part of version one:

```text
ResourceMappingManifestV1
├── schema_version = 1
├── community_id: [u8; 16]
├── base_meta_event_id: [u8; 32]
├── base_project_revision: u64
├── base_projection_generation: u64
└── entries: Vec<ReviewedResourceMappingV1>

ReviewedResourceMappingV1
├── resource_id: [u8; 16]
├── legacy_object_revision: u64
├── legacy_projection_event_id: [u8; 32]
├── legacy_body_digest: [u8; 32]
├── reviewed_v3_payload: CanonicalResourceCutoverV1
├── v3_payload_digest: [u8; 32]
├── guide_document_revision: u64
├── guide_head_event_id: [u8; 32]
├── guide_revision_event_id: [u8; 32]
├── guide_content_digest: [u8; 32]
├── mapping_entry_digest: [u8; 32]
├── reviewed_by_pubkey: [u8; 32]
├── reviewed_at_unix_micros: i64
├── review_digest: [u8; 32]
└── review_signature: [u8; 64]
```

`CanonicalResourceCutoverV1` is exactly:

```text
resource_data { name, resource_kind, summary: Option<String>, guide_document_id:[u8;16] }
context_references = []
```

The empty Context vector is signed. Entries sort by Resource UUID bytes and
duplicates are invalid. A manifest has at most 4,096 entries and its human JSON
envelope is at most 256 MiB before allocation. Human envelope fixed bytes are
lowercase hex and are decoded into fixed arrays before postcard. Strings use
exact UTF-8 bytes without Unicode normalization; `Some("")` is invalid.

Every digest is:

```text
SHA-256(domain || postcard(canonical_value))
```

The exact domains, including terminal NUL, are:

```text
buzz-pv3-legacy-resource-v1\0
buzz-pv3-resource-cutover-payload-v1\0
buzz-pv3-guide-snapshot-v1\0
buzz-pv3-resource-mapping-v1\0
buzz-pv3-resource-review-v1\0
buzz-pv3-resource-manifest-v1\0
```

The Guide snapshot digest covers Document ID/revision/title/summary/Markdown,
not a Relay signature. The mapping digest binds Community/base pins, Resource
identity and legacy pins, final payload digest, and all Guide pins. The review
digest binds the mapping digest, reviewer pubkey, and Unix-microsecond time.
The manifest digest covers its header and complete sorted reviewed entries,
including signatures.

A current direct Human member signs the 32-byte review digest with the existing
Nostr secp256k1 BIP-340 detached signature format. Local signing checks improve
UX; server-side validate and cutover independently verify the pinned membership
snapshot, current eligibility, exact bytes, signature, and every Resource and
Guide pin. Operator authority cannot substitute for Human content review.

Changing serializer, field order, digest boundary, or signature representation
requires a new schema and domain; it cannot mutate v1.

## Durable maintenance state machine

Project View v3 cutover uses an immutable maintenance epoch and this closed
state machine:

```text
normal --begin--> draining --freeze--> frozen
draining --abort(pre-commit, no cutover receipt)--> normal
frozen   --abort(pre-commit, no cutover receipt)--> normal
frozen   --resume(post-commit, verified v3)--> normal
```

`begin` disables Project View and captures exact Assignment, supervisor
binding, runtime, Project revision, projection generation, membership, and
minimum maintenance-protocol baselines. Draining blocks new work/admission and
requires durable Assignment-level and runtime-level quiescence acknowledgments.
Freeze is allowed only when every exact baseline and scheduler claim is safe.

Abort is a pre-commit operation only. It never revives an old runtime identity,
lease, fence, child process, or pool slot. Once a cutover receipt commits there
is no v3-to-v2 rollback in this contract. A fault stays frozen and is handled
by typed forward repair/reprojection/verification; Resume requires verified v3
structural readiness and every later security invalidation resolved by a later
operation. Project View's enabled flag is not a substitute for this fence.

Project Document remains independently available during Project View
maintenance, but a Guide changed after its reviewed manifest pin makes the
cutover exact check fail.

## Golden contract

Shared positive, negative, canonical-byte, digest, and signature fixtures are
under [fixtures/project-document-v1](fixtures/project-document-v1/). They are
the single Stage 0 source for Rust, SDK, CLI, Tauri, and documentation tests.
