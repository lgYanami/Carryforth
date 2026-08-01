NIP-PV
======

Project View
------------

`draft` `optional` `relay`

**Depends on**: NIP-01 (event format and filters), NIP-11 (relay information
document), NIP-42 (authenticated relay access), NIP-45 (COUNT), and NIP-70
(protected events)

## Abstract

This document defines Project View, a relay-scoped current-state model shared by
humans and managed agents in one Buzz Community. A Project View describes the
project profile, goals, semantic roles, plans, stages, requirements, issues,
work, resources, and their typed relationships.

Members submit immutable, signed mutation commands. The relay validates each
command against the current project revision and relationship graph, commits
the canonical state atomically, and publishes relay-signed current-state
projections for reads and live subscriptions.

The protocol uses three event kinds:

- member-signed append-only mutation commands (`kind:44300`);
- relay-signed current object heads (`kind:40903`); and
- a relay-signed project metadata head (`kind:40904`).

The Community selected by the relay from the request host is the project and
tenant boundary. Clients never supply a project or Community identifier in a
mutation.

## Terminology

The key words MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD NOT,
RECOMMENDED, NOT RECOMMENDED, MAY, and OPTIONAL in this document are to be
interpreted as described in BCP 14 when, and only when, they appear in all
capitals.

- **Community**: the server-resolved Buzz tenant. One Community owns at most one
  Project View.
- **project id**: the Community UUID. These terms name the same identity in this
  protocol.
- **member**: a current direct Community member, or a managed agent whose
  verified owner is a current member.
- **mutation**: a member-signed request to initialize, create, update, or delete
  Project View state.
- **canonical state**: the relay's transactional Project View state. Nostr
  projections are a read model of this state, not its source of truth.
- **project revision**: the optimistic-concurrency revision of the entire
  Project View.
- **object revision**: the revision of one stable object identity.
- **projection generation**: the generation of relay-signed projections. It
  changes when projections must be rebuilt or re-signed without changing the
  domain state.
- **head**: the one current, non-retired projection event for a projection
  coordinate.
- **tombstone**: the current object head after logical deletion. It reserves the
  object id without retaining the deleted business body.
- **relay identity**: the public key advertised in the relay's NIP-11 `self`
  field.

## Event Kinds

| Kind | Name | Signer | Storage semantics |
| ---: | --- | --- | --- |
| `44300` | Project View Mutation | Community member | append-only accepted command |
| `40903` | Project View Object | relay identity | one current head per object coordinate |
| `40904` | Project View Meta | relay identity | one current head per Community |

`kind:40903` and `kind:40904` use indexed `d` tags, but they are outside the
NIP-01 `30000 <= n < 40000` addressable range and MUST NOT acquire NIP-33
replacement semantics. Their heads are changed only by the Project View
transaction using `(community, kind, d)`, project revisions, and projection
generations. Generic `(pubkey, kind, d, created_at)` last-write-wins processing
MUST NOT be used.

Clients MUST NOT submit `kind:40903` or `kind:40904`. Relays MUST reject them as
relay-only kinds.

## Project and Object Identity

The relay binds every request to a Community from the request host before
Project View authorization or storage. The bound Community UUID is the project
id and MUST be used for all canonical rows and projection coordinates.

A mutation MUST NOT contain `project_id`, `community_id`, a Community `h` tag,
or any equivalent client-selected tenant value. A relay MUST NOT infer the
project from event content or tags.

Object identifiers have these rules:

- the Project Profile id is assigned by the relay and equals the Community
  UUID;
- all other object ids are client-generated RFC 4122 UUID v4 values;
- an id is unique across all object types in the Project View;
- an id and its object type are immutable; and
- a tombstoned id remains occupied and cannot be reused.

UUIDs on the wire MUST use the canonical lowercase hyphenated representation.
Public keys and event ids MUST be 64-character lowercase hexadecimal strings.
Canonical timestamps in projection content MUST be UTC RFC 3339 strings.

## Domain Vocabulary

### Object types and fields

The `object_type` vocabulary is closed in schema version 1:

| `object_type` | Business fields | Relation fields |
| --- | --- | --- |
| `project_profile` | `name`, `positioning`, `purpose`, `problem`, `scope` | none |
| `goal` | `title`, `desired_outcome`, `directions` | none |
| `role` | `name`, `purpose`, `responsibilities`, `boundaries`, `active` | none |
| `plan` | `title`, `description`, `status` | optional `under_goal_id` |
| `stage` | `title`, `description`, `status` | required `under_plan_id` |
| `requirement` | `title`, `description`, `status`, `priority` | optional `planned_in_stage_id` |
| `issue` | `title`, `description`, `status`, `priority` | optional `planned_in_stage_id`, optional `about` |
| `work` | `title`, `description`, `status`, `priority` | required `handles` |
| `resource` | `name`, `resource_type`, `locator`, `description` | none |

`about` and `handles` are typed references:

```json
{
  "object_type": "requirement",
  "object_id": "984e9ff6-e929-4ab7-a17f-2300155803c3"
}
```

The declared reference type MUST equal the canonical target type. `handles`
MUST target a Requirement or Issue. `about` MAY target any other active object
but MUST NOT target the Issue itself. All non-null relationship targets MUST
exist and be active in the same Project View.

Deleting an object with an active incoming relationship MUST fail. There is no
implicit cascade.

### Enumerations

The version 1 enum values are:

| Vocabulary | Values |
| --- | --- |
| priority | `low`, `normal`, `high`, `urgent` |
| plan status | `draft`, `active`, `paused`, `completed`, `cancelled` |
| stage status | `planned`, `active`, `paused`, `completed`, `cancelled` |
| requirement status | `proposed`, `ready`, `in_progress`, `satisfied`, `withdrawn` |
| issue status | `open`, `in_progress`, `resolved`, `closed` |
| work status | `pending`, `in_progress`, `paused`, `submitted`, `completed`, `cancelled` |
| resource type | `repository`, `document`, `design`, `service`, `environment`, `artifact`, `url` |
| locator type | `url`, `nostr_address`, `nostr_event`, `buzz_deep_link` |

A Resource locator has the following closed shape:

```json
{
  "locator_type": "nostr_address",
  "value": "30617:<repository-pubkey>:<repository-id>"
}
```

Locators are inert data. A relay MUST NOT fetch or resolve one while validating
a mutation.

### Aggregate invariants

An uninitialized Project View has project revision `0`, no objects, and no meta
projection.

An initialized Project View MUST always contain:

- exactly one active Project Profile whose id equals the Community UUID; and
- at least one active Goal.

The Project Profile cannot be created by a normal Create operation and cannot
be deleted. The last active Goal cannot be deleted.

Stage is a stable grouping, not an implicit ordering mechanism. Schema version
1 defines no user-controlled ordering field. Readers use deterministic
`(created_at, id)` ordering when assembling collections.

## Mutation Event

### Outer event

A mutation is a `kind:44300` event signed by the authenticated member making
the change.

It MUST contain exactly these two tags and no others:

```json
[
  ["-"],
  ["t", "buzz-project-view-mutation"]
]
```

There MUST be exactly one NIP-70 protected `-` tag and exactly one matching `t`
tag. In particular, `h`, `d`, `e`, `a`, and `p` tags are forbidden. Operation
type, object type, and object id are read only from the typed content and are
not duplicated in tags.

The event public key MUST equal the authenticated principal. The event is
Community-global, so channel-scoped credentials MUST NOT submit it.

### Content envelope

The content is UTF-8 JSON:

```json
{
  "schema_version": 1,
  "expected_project_revision": 12,
  "request": {
    "type": "update",
    "object_type": "issue",
    "object_id": "7a80c3c2-73d2-4b58-b43d-10947609f9df",
    "patch": {
      "status": "in_progress",
      "planned_in_stage_id": "43a642c4-b3aa-4216-bd76-a59c0730a91a"
    }
  }
}
```

`schema_version` and `expected_project_revision` are required. Version 1
mutation JSON is a closed schema: unknown fields, unknown enum values, and known
fields with the wrong JSON type MUST be rejected.

All revisions and generations are JSON numbers in the inclusive range
`0..=9007199254740991`. Implementations MUST NOT parse them through a lossy
floating-point conversion.

The mutation MUST be applied only when `expected_project_revision` equals the
current canonical project revision. A mismatch is a conflict, not a
last-write-wins update.

### Initialize

Initialize is the only operation allowed on an uninitialized Project View:

```json
{
  "schema_version": 1,
  "expected_project_revision": 0,
  "request": {
    "type": "initialize",
    "profile": {
      "name": "Buzz",
      "positioning": "A shared project surface",
      "purpose": "Coordinate humans and agents",
      "problem": "Project state is fragmented",
      "scope": "Project View v1"
    },
    "goals": [
      {
        "id": "d90bfa24-22df-4602-9a39-7da5a5c09561",
        "title": "Ship Project View",
        "desired_outcome": "Members share one current view",
        "directions": ["Establish the protocol first"]
      }
    ]
  }
}
```

Initialize MUST contain between 1 and 32 Goals. The relay assigns the Profile
id. The Profile and all initial Goals are one atomic mutation: either all are
created at project revision `1`, or none are.

### Create

Create adds one non-Profile object:

```json
{
  "schema_version": 1,
  "expected_project_revision": 12,
  "request": {
    "type": "create",
    "object": {
      "object_type": "plan",
      "id": "773c47e0-c189-47c5-8d9f-0870a2b2a465",
      "title": "MVP",
      "description": "Deliver the first useful slice",
      "status": "active",
      "under_goal_id": null
    }
  }
}
```

The object body is discriminated by `object_type` and contains exactly the
business and relation fields listed in the domain vocabulary. Optional
relations are present as a UUID/reference or JSON `null`. Required relations
MUST be present and non-null.

### Update

Update applies an object-type-specific patch:

```json
{
  "schema_version": 1,
  "expected_project_revision": 12,
  "request": {
    "type": "update",
    "object_type": "issue",
    "object_id": "7a80c3c2-73d2-4b58-b43d-10947609f9df",
    "patch": {
      "status": "in_progress",
      "about": null
    }
  }
}
```

Patch semantics are:

- an absent field is unchanged;
- a non-null value replaces the field;
- `null` clears an optional relation;
- `null` for a required business field or required relation is invalid; and
- `id`, `object_type`, creation time, and creator are not patchable.

An empty patch or a patch whose application produces no semantic change MUST
be rejected and MUST NOT advance either revision.

### Delete

Delete tombstones one active object:

```json
{
  "schema_version": 1,
  "expected_project_revision": 12,
  "request": {
    "type": "delete",
    "object_type": "resource",
    "object_id": "ca09c37f-bcdf-44d4-9e8d-7db890aab4cf"
  }
}
```

Version 1 has no restore and no normal hard-delete operation. Delete reserves
the id permanently, publishes a tombstone head, and retains the accepted
member-signed command as history. Deletion is therefore not a privacy-erasure
mechanism.

Except for Initialize, one mutation changes exactly one canonical object.
Version 1 does not define a generic batch operation.

## Relay-Signed Projections

Projection content is a read model. A client MUST verify the Nostr signature,
the relay identity, the Community/project id, the `d` coordinate, the tag and
content revisions, and the content discriminator before applying it.

### Coordinates

The metadata coordinate is:

```text
project-view:<community-uuid>:meta
```

An object coordinate is:

```text
project-view:<community-uuid>:<object-type>:<object-uuid>
```

Every Project View projection MUST contain exactly one non-empty `d` tag equal
to its derived coordinate. The Community UUID in the coordinate comes from the
server-bound Community, never from client input.

For each `(community, kind, d)` there MUST be at most one current,
non-retired head. Old heads MAY remain soft-retired internally but MUST NOT be
returned by ordinary REQ, query, COUNT, or search.

### Active object projection

An active object head is `kind:40903` with exactly these protocol tags:

```json
[
  ["-"],
  ["d", "project-view:<community-uuid>:issue:<object-uuid>"],
  ["t", "buzz-project-view"],
  ["t", "buzz-project-view-active"],
  ["type", "issue"],
  ["projection_generation", "1"],
  ["revision", "4"],
  ["project_revision", "18"],
  ["e", "<source-command-event-id>", "", "source"]
]
```

Tag order has no meaning, but tag names, values, and cardinalities MUST match
the content. Decimal tag values use canonical unsigned decimal notation with no
sign, whitespace, or leading zero except `"0"`.

The content is:

```json
{
  "schema_version": 1,
  "projection_type": "object",
  "project_id": "<community-uuid>",
  "projection_generation": 1,
  "project_revision": 18,
  "object_revision": 4,
  "source_event_id": "<source-command-event-id>",
  "deleted": false,
  "object": {
    "id": "<object-uuid>",
    "object_type": "issue",
    "created_at": "2026-07-27T10:00:00Z",
    "updated_at": "2026-07-27T10:05:00Z",
    "created_by": "<member-pubkey>",
    "updated_by": "<member-pubkey>",
    "data": {
      "title": "Point reads can under-fetch",
      "description": "Push the d coordinate into SQL before LIMIT",
      "status": "in_progress",
      "priority": "high"
    },
    "relations": {
      "planned_in_stage_id": "<stage-uuid>"
    }
  }
}
```

`object.data` contains the business fields for the declared `object_type`.
`object.relations` contains only non-null relation fields. The outer
`object_type`, the data shape, the relations, the coordinate, and the `type`
tag MUST agree.

### Tombstone object projection

A tombstone is also `kind:40903` and uses the same coordinate. Its common tags
match the active form, except it has:

```json
["t", "buzz-project-view-tombstone"]
```

instead of `buzz-project-view-active`.

Its content is:

```json
{
  "schema_version": 1,
  "projection_type": "object",
  "project_id": "<community-uuid>",
  "projection_generation": 1,
  "project_revision": 19,
  "object_revision": 5,
  "source_event_id": "<source-command-event-id>",
  "deleted": true,
  "object_id": "<object-uuid>",
  "object_type": "issue",
  "deleted_at": "2026-07-27T10:10:00Z"
}
```

A tombstone MUST NOT contain the deleted object's business data, relations, or
resource locator. It replaces the local active head and permanently reserves
the id.

### Metadata projection

The metadata head is `kind:40904`:

```json
[
  ["-"],
  ["d", "project-view:<community-uuid>:meta"],
  ["t", "buzz-project-view"],
  ["t", "buzz-project-view-meta"],
  ["projection_generation", "1"],
  ["project_revision", "18"],
  ["e", "<source-command-event-id>", "", "source"]
]
```

Its content is:

```json
{
  "schema_version": 1,
  "projection_type": "meta",
  "project_id": "<community-uuid>",
  "initialized": true,
  "projection_generation": 1,
  "project_revision": 18,
  "active_object_count": 47,
  "reset": false,
  "changed_heads": [
    {
      "coordinate": "project-view:<community-uuid>:issue:<object-uuid>",
      "event_id": "<new-object-projection-event-id>",
      "object_revision": 4,
      "deleted": false
    }
  ],
  "source_event_id": "<source-command-event-id>",
  "updated_at": "2026-07-27T10:05:00Z"
}
```

`active_object_count` includes the Profile and every active object; it excludes
tombstones. On Initialize, `changed_heads` lists the Profile and all initial
Goals. On a normal successful mutation it lists every object head changed by
that mutation. A client MUST NOT advance its applied project revision until it
has received or fetched every exact event id in `changed_heads`.

A maintenance reprojection increments `projection_generation` without
incrementing `project_revision`. Its new metadata head sets `reset: true`;
`changed_heads` MAY be empty because the generation change invalidates every
cached head. Since a reset has no member command, its source `e` tag and
`source_event_id` are omitted. For `reset: false`, both are required and MUST
match.

An uninitialized Community has no metadata projection. After confirming the
relay advertises Project View support, a client interprets no metadata head as
`initialized: false`.

## Revision, Time, and Atomicity

Initialization changes project revision `0` to `1`. Each later successful
mutation increments the project revision exactly once.

Object creation starts its object revision at `1`. Update and Delete increment
that object's revision exactly once. Other objects retain their existing object
and project revisions.

Canonical `created_at`, `updated_at`, and `deleted_at` values and the projection
event timestamp are relay-assigned after authorization. A mutation's
`created_at` is not canonical domain time.

The following changes MUST commit in one database transaction:

1. the accepted member-signed command;
2. its idempotency receipt;
3. the canonical project and object state;
4. retirement of previous projection heads;
5. the new object projection head or heads; and
6. the new metadata head.

Any failure MUST roll back all six. Publishing committed events to local or
cross-node subscribers happens after commit. If fan-out fails, a client can
recover the same committed state through a query.

Replaying the same accepted event id MUST NOT change state or increment
revisions again. A new event with the same semantic content is a new command
and is still subject to the current expected revision.

## Relay Authorization and Behavior

A relay MUST accept a mutation only after all of the following:

- normal event signature, timestamp, size, admission, and rate-limit checks;
- the event pubkey equals the authenticated principal;
- the principal has `MessagesWrite`;
- the authentication token is Community-global, not channel-scoped;
- the principal is a current member under the definition above;
- the principal and, for a managed agent, its owner are not banned;
- the principal is not write-blocked by timeout;
- Project View is enabled and its schema, signer, and projection generation are
  ready; and
- the exact mutation tags and closed content schema are valid.

These membership rules apply even when a relay is configured to allow other
open-relay behavior. Project Roles are descriptive Project View objects and do
not grant Buzz permissions.

The accepted `kind:44300` command is immutable history. NIP-09 deletion of an
accepted command MUST be rejected; domain undo is represented by a later typed
mutation. Clients also cannot use NIP-09 to retire relay projections.

Project View protocol events MUST NOT trigger workflows, message/thread
counters, or duplicate relay-authored business audit records. The accepted
mutation is the attributable member action.

### Read authorization

Project View is Community-global but not public. A relay MUST expose
`kind:44300`, `kind:40903`, and `kind:40904` only to an authenticated member
with `MessagesRead` and a Community-global credential.

The relay MUST apply the same result-level gate to WebSocket history, live
fan-out, HTTP query, COUNT, id lookup, and any mixed-kind query. A non-member
single-kind Project View query is rejected. A mixed-kind query MUST omit
Project View matches without revealing their existence or count.

Version 1 does not support Project View full-text search. An explicit search
filter targeting a Project View kind is unsupported.

## Capability Discovery

A supporting relay advertises the following NIP-11 extension:

```json
{
  "supported_extensions": ["buzz-project-view-v1"]
}
```

The extension MUST be advertised for a Community host only when Project View is
enabled and its schema, stable relay signer, and current projection generation
are ready. Clients MUST check this capability before sending `kind:44300`.

Until this draft receives an upstream integer NIP number, relays MUST NOT add an
invented number to NIP-11 `supported_nips`.

## Queries and Snapshot Assembly

Clients obtain the expected relay author from NIP-11 `self` and MUST include it
in Project View projection queries.

Read the metadata head:

```json
{"kinds":[40904],"authors":["<nip11-self>"],"limit":2}
```

Read one object head:

```json
{
  "kinds":[40903],
  "authors":["<nip11-self>"],
  "#d":["project-view:<community-uuid>:issue:<object-uuid>"],
  "limit":2
}
```

The limit is `2`, not `1`, so a client can detect the invalid condition of
multiple current heads. It MUST report an integrity error rather than choosing
one by timestamp.

A live subscription uses both kinds:

```json
{"kinds":[40903,40904],"authors":["<nip11-self>"]}
```

Subscribing only to active-object tags is invalid because it misses tombstones.

### Revision-pinned full pagination

A complete view can exceed ordinary relay limits. Version 1 extends HTTP
`POST /query` with one revision-pinned filter:

```json
[
  {
    "kinds": [40903],
    "authors": ["<nip11-self>"],
    "#t": ["buzz-project-view-active"],
    "limit": 500,
    "buzz_project_view": {
      "revision": 18,
      "projection_generation": 1,
      "after": {
        "object_type": "issue",
        "object_id": "<object-uuid>"
      }
    }
  }
]
```

Rules:

- this extension is HTTP-only;
- the request contains exactly one filter and cannot combine Project View
  pagination with search, feed, or channel-window extensions;
- outer `kinds`, `authors`, and `#t` MUST exactly match the example;
- `ids`, `#d`, `since`, `until`, offset paging, `before_id`, and unknown outer
  fields are forbidden;
- `revision` and `projection_generation` come from the same verified metadata
  head;
- `limit` is in `1..=500`;
- absent `after` means the first page;
- ordering is `(object_type ASC, object_id ASC)`; and
- a revision or generation mismatch returns HTTP `409`.

The response remains a standard array of relay-signed Nostr events. A short
page marks exhaustion. A missing canonical projection is an internal
consistency error, not an empty page.

To assemble a consistent snapshot, a client:

1. reads and verifies metadata generation `G`, revision `R`, and active count
   `N`;
2. reads every active object page pinned to `(G, R)`;
3. verifies signatures, coordinates, unique ids, generation `G`, and
   `projection.project_revision <= R`;
4. reads metadata again; and
5. accepts the snapshot only when both metadata reads are `(G, R)` and exactly
   `N` unique active objects were read.

A client MAY retry a changing snapshot a bounded number of times with backoff.
It MUST NOT return a mixed-revision view as complete.

### Live ordering

For snapshot plus live updates, a client opens and buffers the live
subscription before fetching a snapshot. After applying `(G, R)`, it discards
older buffered events and groups later events by
`(projection_generation, project_revision)`.

Object and metadata events for one revision can arrive in either order. The
client applies a revision atomically only after the metadata event and every
exact `changed_heads.event_id` are present. A revision gap, missing head,
generation change, signer change, or reconnect requires an exact recovery read
or a new snapshot. Event arrival time is never the ordering authority.

## Mutation Responses and Errors

Successful submission uses the standard Nostr `OK` response. Its message is a
Buzz command response:

```text
response:{"project_revision":13,"object_id":"<uuid>","object_revision":2,"deleted":false}
```

Initialize MAY omit a singular `object_id` and `object_revision` because it
changes multiple heads. A duplicate retry of an accepted event returns the same
stored response only after current authentication, membership, feature, and
readiness checks still pass.

Failure prefixes are:

| Condition | Nostr `OK false` prefix | HTTP status |
| --- | --- | ---: |
| malformed field, tag, relation, or no-op | `invalid:project_view:<code>` | 400 |
| membership, scope, ban, timeout, or credential restriction | `restricted:` | 403 |
| revision or initialization-state conflict | `conflict:project_view:<code>` | 409 |
| unsupported schema or wire capability | `unsupported:project_view:<code>` | 400 |
| disabled or not-ready schema/signer/generation | `unavailable:project_view:<code>` | 503 |
| internal persistence or signing failure | `error:internal` | 500 |

Clients MUST NOT automatically retry 400, 403, or 409 responses. A client MAY
retry an uncertain network result or 503 with bounded backoff while the event
timestamp remains acceptable, but it MUST resend the exact same signed event.

## Input Limits

Version 1 limits are measured in UTF-8 bytes:

| Input | Limit |
| --- | ---: |
| mutation content | 64 KiB |
| `name` or `title` | 256 bytes |
| one long-text field | 32 KiB |
| one string list | 64 items |
| one string-list item | 512 bytes |
| Resource locator value | 4096 bytes |
| Initialize Goals | 1–32 |
| mutation JSON nesting depth | 16 |

Required text must be non-empty after Unicode whitespace trimming. Validation
does not otherwise normalize or rewrite caller text.

A URL locator MUST parse as a URL and MUST NOT contain user information, an
embedded password, or control characters. Other locator types MUST be non-empty
and contain no control characters.

## Security and Privacy Considerations

Project View is readable by every authorized Community member. It is not a
secret store. Clients MUST NOT place private keys, passwords, bearer tokens, or
restricted infrastructure credentials in any object.

The NIP-70 protected tag reduces replay of a member-signed mutation to unrelated
relays, but it does not itself provide authorization, idempotency, or deletion
protection. Those are relay responsibilities.

Clients MUST trust only projections signed by the current NIP-11 relay identity.
A stale or foreign signer, a mismatched Community coordinate, or inconsistent
tag/content metadata is an integrity failure.

Including the Community UUID in every projection coordinate prevents a shared
relay signer from creating coordinate collisions across hosted Communities.
The database tenant key remains authoritative; the coordinate is
defense-in-depth and a client verification input.

Relays SHOULD log only bounded identifiers and error codes for Project View
operations, not object bodies or complete Resource locators.

## Non-Goals

Version 1 does not define:

- a second project identity separate from the Community;
- field-level access control or permissions derived from Project Roles;
- arbitrary graph edges or custom object types;
- generic multi-object batch mutations;
- implicit cascade delete, restore, or privacy hard-delete;
- ordered Stages;
- full-text or semantic Project View search;
- a separate receipt event;
- client-authored current-state projections; or
- offline last-write-wins conflict resolution.

These capabilities require an explicit future schema or capability version.

## Compatibility

Mutation schema version 1 is closed. Any new client-to-server field, enum value,
or write semantic requires a new advertised schema/capability version.

Projection readers SHOULD ignore unknown optional fields within a known major
schema version, but MUST reject wrong types or contradictions in known identity,
revision, signer, coordinate, and discriminator fields.

Relays and clients that do not advertise or understand
`buzz-project-view-v1` ignore these kinds according to their normal unknown-kind
policy.
