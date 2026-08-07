# NIP-PCE: Project Context Edges

`draft` `optional`

## Abstract

Project Context Edges represent explanatory context spanning an exact,
unordered set of two or more Project coordinates. The relation is an
undirected hyperedge. One or more ordinary Project Documents carry the actual
explanation; the edge itself does not duplicate Markdown content.

For a given Project and exact coordinate set there is exactly one edge. An edge
may contain multiple Context Documents, while one Context Document may belong
to at most one active edge. `{A,B}` and `{A,B,C}` are different edges.

This NIP defines the v2 coordinate union, deterministic edge identity,
member-signed attach/detach commands, relay-signed binding and metadata
projections, and their strict verification rules.

## Status and scope

The protocol kinds and wire contract are registered. Until a relay advertises
`buzz-project-context-edge-v2` and has a ready canonical Context catalog, it
MUST reject exclusive reads and writes as unavailable and MUST exclude these
kinds from wildcard, mixed-kind, IDs-only, count, search-candidate, point-read,
and live-subscription results.

This NIP does not define Project Context inference. A relay does not decide
that context is missing, stale, conflicting, or incorrect. Humans and Agents
discover or create that meaning while doing work and explicitly attach,
detach, or update the referenced Project Documents.

## Event kinds

| Kind | Name | Author | Role |
|---:|---|---|---|
| `44302` | Project Context command | Community member | append-only attach/detach request |
| `40908` | Project Context binding | Relay | current active/deleted head for one Context Document binding |
| `40909` | Project Context meta | Relay | current catalog observation boundary |

Kinds `40908` and `40909` are relay-only. They use indexed `d` tags as domain
query coordinates but are not NIP-33 last-write-wins events. Replacement is
controlled by the Project Context revision protocol. All three kinds are
Community-private and Community-global; none uses an `h` tag.

## Coordinate model

The v2 union is closed:

```json
{
  "coordinate_type": "project_view_object",
  "object_type": "requirement",
  "object_id": "0fd3a16e-4da4-48c1-aa6a-63b3661091d0"
}
```

```json
{
  "coordinate_type": "document",
  "document_id": "9c23f672-a397-42d1-b933-104ba2674f26"
}
```

```json
{
  "coordinate_type": "meeting",
  "meeting_id": "0ed366aa-6f94-4eff-83db-b8bf081fbf35"
}
```

`object_type` uses the Project View vocabulary:

1. `project_profile`
2. `goal`
3. `role`
4. `plan`
5. `stage`
6. `requirement`
7. `issue`
8. `work`
9. `resource`

IDs are RFC 4122 UUID v4 values. A `project_profile` coordinate's `object_id`
MUST equal the host-derived Project ID.

### Canonical order

Every edge carries at least two distinct coordinates. The canonical order is:

1. Project View coordinates before Document coordinates, and Document
   coordinates before Meeting coordinates;
2. Project View coordinates by the explicit object-type order above, then UUID
   bytes;
3. Document coordinates by UUID bytes;
4. Meeting coordinates by UUID bytes.

Senders MUST canonicalize before signing. Receivers MUST reject duplicate,
undersized, or non-canonically ordered sets. They MUST NOT silently sort a
received signed command or projection.

This explicit order is protocol data. Implementations MUST NOT derive it from
language enum discriminants or serialized string order.

### Query tag values

Each binding carries one `c` tag per coordinate, in canonical order:

```text
pv:<project-uuid>:<object-type>:<object-uuid>
document:<project-uuid>:<document-uuid>
meeting:<project-uuid>:<meeting-uuid>
```

These tags support incident queries. Their values are derived from verified
content and the host-derived Project; they are not independent authority.

## Deterministic edge identity

The edge key is:

```text
SHA-256(
  "buzz-project-context-edge-v1\0" ||
  project_uuid_bytes ||
  coordinate_count_u32_be ||
  canonical_coordinate_bytes...
)
```

Each Project View coordinate is encoded as:

```text
0x00 || object_type_rank_u8 || object_uuid_bytes
```

Each Document coordinate is encoded as:

```text
0x01 || document_uuid_bytes
```

Each Meeting coordinate is encoded as:

```text
0x02 || meeting_uuid_bytes
```

The edge-key algorithm intentionally retains the v1 domain separator. Schema
v2 appends the previously unused `0x02` family rank, so every Project View /
Document-only edge keeps its existing key byte-for-byte.

Object ranks are zero-based in the order listed above. The wire spelling of an
edge key is exactly 64 lowercase hexadecimal characters.

The Project ID participates in the hash, so equal coordinate bytes in two
Projects have different edge keys. Coordinate count and fixed variant bytes
make `{A,B}` distinct from `{A,B,C}` and prevent concatenation ambiguity.

## Commands

Kind `44302` carries exactly these tags, in this order:

```json
[
  ["-"],
  ["t", "buzz-project-context-edge-command"]
]
```

The closed content shape is:

```json
{
  "schema_version": 2,
  "expected_context_revision": 12,
  "acting_assignment_id": "151f2347-7d24-41a0-ab0d-f272e84fcf88",
  "runtime_fence": {
    "runtime_id": "74ad5e95-903b-4488-ac19-d95a73fa62d4",
    "runtime_epoch": 4
  },
  "request": {
    "type": "attach",
    "coordinates": [],
    "context_document_id": "9c23f672-a397-42d1-b933-104ba2674f26"
  }
}
```

`request.type` is `attach` or `detach`. `acting_assignment_id` and
`runtime_fence` are optional, but MUST either both be absent or both be
present. Explicit JSON `null` is non-canonical and rejected.

`expected_context_revision` is a global Project Context catalog CAS value.
Relays reject a command when it differs from current canonical state. Accepted
command event IDs are stable replay identities; an adapter returns the prior
receipt for an already accepted command only after ordinary host, credential,
membership, and restriction gates pass.

### Attach

Attach requires every Project View / Document coordinate and the Context
Document to be active at the transaction-locked check. A Meeting coordinate
MUST resolve in the same host-derived Project to a verified terminal
Create-State-End chain whose normalized outcome is `closed` or `aborted`.
Active Meetings, ordinary Channel UUIDs, foreign/missing Meetings, and invalid
terminal chains are rejected. The Document MUST have no other active Context
Edge binding. The first Document creates the edge; subsequent Documents join
the same deterministic edge.

### Detach

Detach requires the Document's exact active binding and exact coordinate set.
It does not require its coordinates or target Document to remain active, which
allows cleanup after coordinate tombstones. The last Document removes the
active edge. Deleted binding transport state retains the edge key and canonical
coordinates.

Meeting lifecycle state is checked only for a new attach. Detach MUST use the
persisted canonical exact set and remain possible after Meeting archival or a
later hydration failure.

## Binding projection

Kind `40908` content is closed:

```json
{
  "schema_version": 2,
  "projection_type": "context_edge_binding",
  "project_id": "3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77",
  "projection_generation": 1,
  "context_revision": 1,
  "edge_key": "5fd64dcb2a0aa7e37b696806be6c815df9dc3f3766b1613a89746269cde139fc",
  "coordinates": [],
  "context_document_id": "9c23f672-a397-42d1-b933-104ba2674f26",
  "state": "active",
  "source_event_id": "6b9b34d6048d5782acd918e36565cbb9425bcbbf90ded11d287384af45bf2be6",
  "updated_at": "2027-01-15T08:00:02Z"
}
```

`state` is `active` or `deleted`. Both variants retain identical identity
fields. `created_at` MUST equal `updated_at` at whole-second precision.

The exact tag sequence is:

```text
["-"]
["d", "project-context-edge:<project>:binding:<context-document>"]
["t", "buzz-project-context-edge"]
["t", "binding"]
["s", "active" | "deleted"]
["g", "project-context-edge:<project>:<edge-key>"]
["c", "<canonical-coordinate-tag>"]  repeated in canonical order
["projection_generation", "<canonical-decimal>"]
["context_revision", "<canonical-decimal>"]
["e", "<source-command-event-id>", "", "source"]
```

The `d` identity means one current binding event exists per Context Document.
Its content identifies the one edge currently owning that Document. The `g`
identity groups all binding heads belonging to the same exact edge.

## Metadata projection

Kind `40909` content is:

```json
{
  "schema_version": 2,
  "projection_type": "context_meta",
  "project_id": "3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77",
  "projection_generation": 1,
  "context_revision": 1,
  "active_edge_count": 1,
  "bound_document_count": 1,
  "reset": false,
  "changed_bindings": [],
  "source_event_id": "6b9b34d6048d5782acd918e36565cbb9425bcbbf90ded11d287384af45bf2be6",
  "updated_at": "2027-01-15T08:00:02Z"
}
```

The exact base tag sequence is:

```text
["-"]
["d", "project-context-edge:<project>:meta"]
["t", "buzz-project-context-edge"]
["t", "meta"]
["projection_generation", "<canonical-decimal>"]
["context_revision", "<canonical-decimal>"]
```

An ordinary incremental observation sets `reset:false`, has a positive Context
revision, contains exactly one `changed_bindings` entry, and appends:

```text
["e", "<source-command-event-id>", "", "source"]
```

The changed entry contains `context_document_id`, `edge_key`, canonical
`binding_coordinate`, exact `binding_event_id`, and `state`. It MUST bind the
same Relay signer, Project, generation, Context revision, and source command as
the referenced binding event.

A bootstrap or full reprojection observation sets `reset:true`, has no source
event, and carries an empty `changed_bindings` array. Revision zero is reserved
for the untouched empty catalog. A reset at a later revision establishes a new
complete observation boundary; binding projections in that generation may
have revisions less than or equal to the reset meta revision.

Catalog invariants are:

- `active_edge_count <= bound_document_count`;
- edge and binding counts are either both zero or both non-zero;
- `projection_generation` is positive;
- all revisions and counts are JavaScript-safe integers.

## Receipt

An accepted command has a closed stable receipt containing:

```text
schema_version
change_id
actor
acting_assignment_id (optional)
operation
expected_context_revision
context_revision
edge_key
edge_state
edge_document_count
context_document_id
accepted_at
```

`edge_state:active` requires a positive `edge_document_count`.
`edge_state:deleted` requires zero. Projection event IDs are intentionally not
part of the business receipt.

## Limits and strict parsing

V2 freezes these bounds:

```text
minimum edge coordinates       2
maximum command content bytes  65,536
maximum projection content     65,536
maximum command JSON depth     16
maximum safe revision/count    9,007,199,254,740,991
```

There is no independent coordinate-count or Documents-per-edge cap. The byte
bounds are the authoritative limit and keep future coordinate variants
extensible without a second limit vocabulary.

Parsers reject unknown fields, unknown union variants, duplicate coordinates,
non-canonical coordinate order, explicit `null` optionals, noncanonical edge
hex, invalid UUIDs, mismatched derived keys, extra/reordered tags, wrong kind,
wrong signer, wrong Project, timestamp mismatch, and cross-observation
pointers.

Before canonical commit, a relay MUST validate:

1. the signed command content limit;
2. every derived projection content limit;
3. every complete signed Nostr `EVENT` frame against its configured WebSocket
   frame limit.

The third check is distinct because repeated `c` tags, signatures, and the
frame envelope are not included in the content-only limit.

## Query semantics

After capability readiness, clients can derive three operations without
changing edge identity:

- `exact({A,B})`: derive the edge key and query active kind `40908` bindings by
  `g`;
- `incident(A)`: query active bindings by one canonical `c` tag;
- `contains-all(Q)`: obtain incident/all candidates and retain edges for which
  `Q` is a subset of the verified canonical coordinate set.

Clients group verified binding heads by edge key and MUST reject a group whose
coordinate sets differ or whose Context Document IDs repeat. Binding content
does not include Markdown; callers hydrate only the necessary Project Document
bodies on demand.

A Meeting coordinate is hydrated metadata-first. Its stable identity contains
only `meeting_id`; title, final Board, participant roster, Speech, action state,
and lifecycle evidence are observations from the Meeting domain rather than
edge identity. A client MAY show bounded terminal metadata in Context results,
but MUST fetch the full Meeting record on demand.

## Privacy

Commands and projections reveal coordinate, Document, and Meeting identities
and are therefore Community-private. Authentication alone is insufficient:
readers must satisfy the relay's current Community principal policy and use a
global read credential. Meeting roster membership is an action boundary, not
an additional Context read boundary. Relay implementations MUST protect
historical REQ, live fan-out, COUNT, HTTP query/count, IDs-only and kindless
filters, point reads, search/fallback candidates, and wildcard filters.

A relay that knows the kinds but lacks a ready canonical Context capability
MUST fail closed. Registration is not authorization and is not readiness.

Context Documents remain untrusted project content. Their text does not gain
system-instruction priority, grant permissions, or execute commands merely
because it is attached to an edge.

## Shared fixtures

Normative v2 interoperability fixtures live in
`docs/nips/fixtures/project-context-edge-v2/`. They freeze canonical Meeting
mixed-set attach and detach command content, active/deleted binding events,
incremental/reset meta events, receipt bytes, event IDs, and deterministic edge
keys. Production SDK parsers consume these same fixtures in tests.

The frozen v1 fixtures remain under
`docs/nips/fixtures/project-context-edge-v1/` solely for operator-controlled
v1-to-v2 reprojection verification. Ordinary v2 command and projection parsers
MUST reject them; relays MUST NOT advertise or dual-write the v1 capability.
