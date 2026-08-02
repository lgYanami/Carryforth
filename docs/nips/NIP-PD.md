# NIP-PD: Buzz Project Document v1

`draft` `optional`

## Abstract

Project Document is Buzz's Community-private, revisioned Markdown catalog. It
stores operational guides and other durable project knowledge outside chat and
outside Project View's structural object graph. A stable Document identity has
a lightweight current head and immutable full-snapshot revisions. Writes use
member-signed commands and per-Document compare-and-swap; accepted state is
materialized as Relay-signed projections.

This document freezes the v1 wire contract. Registration of these kinds does
not enable the capability. A Relay MUST keep submission and reads fail closed
until the database, authorization, projection, and privacy readiness gates are
implemented and `buzz-project-document-v1` is advertised.

## Capability and privacy boundary

The NIP-11 capability name is exactly:

```text
buzz-project-document-v1
```

Project Documents are host-derived Community-global objects. They are not
Channel objects and never use an `h` tag. Commands, heads, revisions, metadata,
REQ, COUNT, HTTP query results, subscriptions, and cross-pod fan-out are all
Community-private. An event ID or coordinate is not read authorization.

The capability may be enabled only for a Project View schema 2 or 3 Community
with a stable Relay signer and verified canonical/projection parity. Project
Document remains an independent capability and revision domain; this readiness
dependency does not merge it into Project View.

## Kinds

| Kind | Meaning | Signer |
|---:|---|---|
| `44301` | append-only Project Document command | current Community member |
| `40905` | current lightweight Document head | Relay only |
| `40906` | immutable full snapshot or bodyless tombstone revision | Relay only |
| `40907` | current Document catalog metadata | Relay only |

Kinds `40905`, `40906`, and `40907` carry an indexed `d` tag for point queries,
but are not NIP-33 last-write-wins events. Replacement is controlled by the
canonical Document revision, catalog revision, and projection generation.
Clients MUST reject a projection not signed by the currently verified Relay
projection key and generation.

## Common canonical rules

- UUID text is lowercase RFC 4122 form. Client-owned Document IDs are UUID v4,
  RFC 4122 variant, non-nil, and never reusable, including after deletion.
- Event IDs and public keys are 64 lowercase hexadecimal characters.
- Revisions and generations are JSON unsigned integers in
  `0..=9007199254740991`; fields described as positive start at one.
- Decimal tag values have no sign, whitespace, or leading zero, except the
  canonical value `"0"`.
- Times are canonical UTC RFC 3339. A projection event's Nostr `created_at`
  equals the Unix-second component of its canonical projection update time.
- Closed JSON values reject unknown and missing fields. Optional fields are
  omitted; explicit `null` is non-canonical and rejected.
- All string limits count UTF-8 bytes. Strings reject NUL.
- Exact tags mean exact ordered sequence for builders and golden bytes. A
  verifier may compare as a multiset only if it separately rejects duplicate
  tags; tag order does not convey business meaning.

## Limits

| Value | Maximum |
|---|---:|
| command JSON content | 65,536 bytes |
| parsed JSON nesting depth | 16 |
| title | 256 bytes |
| summary | 4,096 bytes |
| `content_markdown` | 49,152 bytes |

Title is non-empty and has no leading or trailing whitespace. An empty summary
is non-canonical and must be omitted. Markdown may be empty and is retained
byte-for-byte, including whitespace and line endings. JSON escaping still
counts toward the 65,536-byte command limit.

## Identity and coordinates

The server-resolved Community UUID is also the Project UUID. Coordinates are:

```text
head     = project-document:<project-uuid>:<document-uuid>
revision = project-document:<project-uuid>:<document-uuid>:revision:<decimal>
meta     = project-document:<project-uuid>:meta
```

Long-lived business references use `document_id`, or `document_id` plus
`document_revision` for a pin. Coordinates and projection event IDs identify
rebuildable signed materializations, not the business identity.

## Command event

A kind `44301` command has exactly these tags:

```json
[
  ["-"],
  ["t", "buzz-project-document-command"]
]
```

The signed content is a closed `ProjectDocumentCommand`:

```json
{
  "schema_version": 1,
  "expected_document_revision": 0,
  "request": {
    "type": "create",
    "document_id": "9c23f672-a397-42d1-b933-104ba2674f26",
    "title": "Buzz repository guide",
    "summary": "Clone, initialize, and verify this repository.",
    "content_markdown": "# Repository\n\nUse the checked-in toolchain."
  }
}
```

Update replaces the complete snapshot:

```json
{
  "schema_version": 1,
  "expected_document_revision": 7,
  "acting_assignment_id": "151f2347-7d24-41a0-ab0d-f272e84fcf88",
  "runtime_fence": {
    "runtime_id": "74ad5e95-903b-4488-ac19-d95a73fa62d4",
    "runtime_epoch": 4
  },
  "request": {
    "type": "update",
    "document_id": "9c23f672-a397-42d1-b933-104ba2674f26",
    "title": "Buzz repository guide",
    "summary": "Clone, initialize, and verify this repository.",
    "content_markdown": "# Repository\n\nRun `just ci` before review."
  }
}
```

Delete carries no business body:

```json
{
  "schema_version": 1,
  "expected_document_revision": 8,
  "request": {
    "type": "delete",
    "document_id": "9c23f672-a397-42d1-b933-104ba2674f26"
  }
}
```

Create requires expected revision zero and commits revision one. Update and
Delete require a positive exact current revision and commit exactly the next
revision. An Update identical to the current title, summary, and Markdown is
`invalid:project_document:no_change`. A deleted identity cannot be recreated,
updated, deleted again, or restored in v1.

A Human command omits both `acting_assignment_id` and `runtime_fence`. A
managed Agent command includes both and uses the shared wire-neutral
`RuntimeFence { runtime_id, runtime_epoch }`. Authorization requires the exact
active Assignment, runtime ID, and epoch; either field alone is invalid.

The event signer is the actor. Event `created_at` is checked for ingest
freshness but never becomes canonical business time.

## Stable receipt

The Relay's existing write response remains:

```json
{
  "event_id": "<command-event-id>",
  "accepted": true,
  "message": "response:{...}"
}
```

The JSON after `response:` is the exact closed receipt:

```json
{
  "schema_version": 1,
  "change_id": "<command-event-id>",
  "actor": "<verified-command-pubkey>",
  "acting_assignment_id": "151f2347-7d24-41a0-ab0d-f272e84fcf88",
  "operation": "update",
  "document_id": "9c23f672-a397-42d1-b933-104ba2674f26",
  "expected_document_revision": 7,
  "document_revision": 8,
  "catalog_revision": 31,
  "state": "active",
  "accepted_at": "2026-07-27T10:05:00Z"
}
```

`acting_assignment_id` is omitted for Human commands. Create and Update commit
`state:"active"`; Delete commits `state:"deleted"`. The receipt binds stable
business facts and deliberately omits head, revision, and meta event IDs.
Signer rotation may replace those pointers without changing an accepted
receipt. A replay of the byte-identical accepted command returns the same
receipt after current security and readiness gates pass.

## Current head projection

Kind `40905` active tags are exactly:

```json
[
  ["-"],
  ["d", "project-document:<project-uuid>:<document-uuid>"],
  ["t", "buzz-project-document"],
  ["t", "buzz-project-document-head"],
  ["t", "buzz-project-document-active"],
  ["projection_generation", "1"],
  ["catalog_revision", "31"],
  ["document_revision", "8"],
  ["e", "<revision-event-id>", "", "revision"],
  ["e", "<command-event-id>", "", "source"]
]
```

Active content is:

```json
{
  "state": "active",
  "schema_version": 1,
  "projection_type": "document_head",
  "project_id": "<project-uuid>",
  "projection_generation": 1,
  "catalog_revision": 31,
  "document_id": "<document-uuid>",
  "document_revision": 8,
  "title": "Buzz repository guide",
  "summary": "Clone, initialize, and verify this repository.",
  "created_at": "2026-07-27T10:00:00Z",
  "created_by": "<creator-pubkey>",
  "updated_at": "2026-07-27T10:05:00Z",
  "updated_by": "<editor-pubkey>",
  "revision_coordinate": "project-document:<project-uuid>:<document-uuid>:revision:8",
  "revision_event_id": "<revision-event-id>",
  "source_event_id": "<command-event-id>"
}
```

A deleted head replaces the active/tombstone tag with:

```json
["t", "buzz-project-document-tombstone"]
```

Its content uses `state:"deleted"`, retains identity, generation, catalog and
Document revisions, `created_at`, `created_by`, `deleted_at`, `deleted_by`, the
tombstone revision coordinate/event, and the source event. It cannot contain
title, summary, or Markdown.

## Immutable revision projection

Kind `40906` active tags are exactly:

```json
[
  ["-"],
  ["d", "project-document:<project-uuid>:<document-uuid>:revision:<decimal>"],
  ["t", "buzz-project-document"],
  ["t", "buzz-project-document-revision"],
  ["t", "buzz-project-document-active"],
  ["projection_generation", "1"],
  ["catalog_revision", "31"],
  ["document_revision", "8"],
  ["e", "<command-event-id>", "", "source"]
]
```

Active content is:

```json
{
  "state": "active",
  "schema_version": 1,
  "projection_type": "document_revision",
  "project_id": "<project-uuid>",
  "projection_generation": 1,
  "catalog_revision": 31,
  "document_id": "<document-uuid>",
  "document_revision": 8,
  "title": "Buzz repository guide",
  "summary": "Clone, initialize, and verify this repository.",
  "content_markdown": "# Repository\n\nRun `just ci` before review.",
  "created_at": "2026-07-27T10:00:00Z",
  "created_by": "<creator-pubkey>",
  "revision_at": "2026-07-27T10:05:00Z",
  "revision_by": "<editor-pubkey>",
  "source_event_id": "<command-event-id>"
}
```

A tombstone revision uses the tombstone tag and `state:"deleted"`. It retains
the common fields, creation provenance, `revision_at`, `revision_by`, and
source event, but cannot contain title, summary, or Markdown. Ordinary updates
never retire old active revision projections. Only a controlled signer
generation rebuild retires projections from the old generation.

## Catalog metadata projection

Kind `40907` incremental tags are exactly:

```json
[
  ["-"],
  ["d", "project-document:<project-uuid>:meta"],
  ["t", "buzz-project-document"],
  ["t", "buzz-project-document-meta"],
  ["projection_generation", "1"],
  ["catalog_revision", "31"],
  ["e", "<command-event-id>", "", "source"]
]
```

Incremental content is:

```json
{
  "schema_version": 1,
  "projection_type": "document_meta",
  "project_id": "<project-uuid>",
  "initialized": true,
  "projection_generation": 1,
  "catalog_revision": 31,
  "active_document_count": 12,
  "reset": false,
  "changed_heads": [
    {
      "head_coordinate": "project-document:<project-uuid>:<document-uuid>",
      "head_event_id": "<head-event-id>",
      "document_id": "<document-uuid>",
      "document_revision": 8,
      "revision_event_id": "<revision-event-id>",
      "deleted": false
    }
  ],
  "source_event_id": "<command-event-id>",
  "updated_at": "2026-07-27T10:05:00Z"
}
```

An ordinary command has exactly one changed head and a source. A reset has
`reset:true`, an empty `changed_heads`, and omits `source_event_id` and its
source tag. Reset covers signer reprojection and the initialized empty-catalog
bootstrap. Bootstrap additionally has catalog revision zero and active count
zero. Reset does not advance a business catalog revision merely because a
projection generation changed.

## Strict projection verification

A verifier checks the signature before consuming content and then checks all
of the following as one contract:

1. expected Relay signer, event kind, and Nostr `created_at`;
2. closed JSON subtype and lifecycle shape;
3. expected host-derived Project UUID;
4. exact tag sequence/cardinality and canonical decimal text;
5. coordinate identity and content/tag generation and revision parity;
6. source pointers in content and marked `e` tags;
7. for a head, revision coordinate, revision event ID, and marked revision tag;
8. for metadata, changed-head coordinate/event/revision binding;
9. active and tombstone business-field exclusion rules.

An unexpected or duplicate tag, alternate Project coordinate, wrong signer,
non-canonical decimal, cross-generation pointer, or unknown JSON field fails
closed. Raw subscription payloads are only invalidation hints until this
verification completes.

## Revision, transaction, and deletion semantics

`document_revision` is the write CAS for one identity. `catalog_revision` is a
monotonic catalog observation used for pagination, metadata invalidation,
projection parity, and cache keys; clients never submit it as write CAS.

The command event, stable receipt, canonical current row, immutable revision,
head, revision event, and metadata event commit atomically. A commit allocates
one Document revision and one catalog revision. A network failure after an
ambiguous write returns `delivery_unknown`; clients must not invent a new
command until they reconcile the signed command ID.

Delete appends a bodyless tombstone. Historical active revisions remain
readable by exact pin. Delete is rejected while an active Resource Guide or
live Document Context Reference targets the Document. Pinned Context
References do not block deletion, and a tombstone revision cannot itself be a
pinned target. Delete is not a compliance erase operation.

## Reads

- catalog observation reads kind `40907`;
- active list reads active kind `40905` heads without Markdown;
- current get verifies a head and its exact referenced kind `40906` event;
- pinned get reads one exact revision coordinate;
- history is bounded and paginated over immutable revisions.

Generic query surfaces cannot redact signed revision content, so list paths
must query heads rather than revisions. NIP-50 search over Project Document
kinds is rejected in v1. All read paths apply current credential, membership,
ban, capability, and projection-readiness gates before receipt or event lookup.

## Errors

Stable server classes are:

```text
invalid:project_document:<reason>
conflict:project_document:<reason>
restricted:project_document:<reason>
unavailable:project_document:<reason>
unsupported:project_document:<reason>
error:project_document:<reason>
```

Frozen v1 reasons are:

```text
invalid_json content_too_large invalid_document_id invalid_snapshot no_change
revision_target revision id_exists still_referenced snapshot_changed
global_credential_required membership_required assignment_required runtime_fence
disabled not_ready stable_signer schema
```

Adapters must not include Markdown, summary, or suspected secrets in error
text or telemetry.

## Signer rotation boundary

v1 signer rotation is an operator maintenance operation, not an automatic
background key switch. It increments `projection_generation`, rebuilds every
current head, every retained immutable revision, and reset metadata with one
stable signer, verifies canonical/event-pointer parity, then atomically makes
the new generation readable. Ordinary readers never mix generations. Old
generation projections are retired from normal query surfaces only after the
new reset boundary is durable and verified. Business revisions, catalog
revisions, command events, and stable receipts do not change.

## Shared golden fixtures

The normative Stage 0 examples and negative cases live in:

```text
docs/nips/fixtures/project-document-v1/
```

Rust domain/SDK tests, CLI tests, Tauri tests, and documentation must consume
these bytes rather than maintain independent copies. The fixture README lists
the fixed keys, UUIDs, timestamps, and expected digest/event IDs.
