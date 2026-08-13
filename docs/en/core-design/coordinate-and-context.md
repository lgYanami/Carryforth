# Project Context: Coordinates First, Context Second

> This document explains a core Carryforth design choice: why project context does not begin by
> storing a block of “related text,” but instead begins with stable coordinates and then preserves
> precise, revisable relational context between those coordinates.
>
> “Coordinate context” and “relational context” are conceptual layers used in this document to
> explain the design. They are not two new wire schemas, database tables, or permission domains.
> For the exact domain contract, see the
> [Project Context domain specification](../../stage/project-context/project-context.md).

## 1. Core judgment

> Context is not knowledge that exists independently of an object. Context is always “about
> something,” and it is valid only within some scope. That “something” must first be stably
> identifiable, verifiable, and readable before the Project can accurately preserve “why these
> things are related.”

In Carryforth, that “something” is a **Coordinate**.

Objects such as Requirements, Work, Resources, Documents, and Meetings first acquire stable
identities inside the Project. A Coordinate, together with the canonical content it identifies in
its source domain, forms the basic context through which an Agent understands one object.

When information is valid only for several objects taken together, Project Context uses an
undirected Edge / Hyperedge to anchor the exact coordinate set. Versioned Project Documents then
carry the reasons, dependencies, effects, exceptions, and boundaries between those coordinates.

```text
Stable Coordinate
  │
  ├── Return to the source domain and read current canonical content ── Coordinate Context
  │
  └── Form an exact set with other Coordinates
          │
          ├── Edge / Hyperedge: exactly what the relationship applies to
          └── Context Documents: why they are related ──────────────── Relational Context
```

The goal is not to build a knowledge graph that automatically understands the entire Project. It
is to let Humans and Agents continually discover, read, verify, and maintain open-ended semantics
around stable project objects.

## 2. What context lacks first is not more text, but explicit objects

Projects readily accumulate notes like this:

> The frontend adaptation depends on the new authorization API, and the old state must be retained
> during cutover.

This sounds useful, but a later member cannot determine:

- which Work “frontend adaptation” refers to;
- which Requirement or Resource “authorization API” refers to;
- whether the statement applies to the current design or a retired one;
- which object changes should cause the statement to be reviewed again;
- whether the reader is authorized to access the other content it implies.

Adding more summaries, tags, or vectors does not automatically solve these problems. Without stable
objects, retrieval can find “similar text,” but it cannot reliably answer “what does this refer to,”
“is it still current,” or “where should a correction be written back?”

Carryforth therefore first models things that must persist in the Project as referenceable objects,
and only then allows relational context to be established around them.

## 3. What a Coordinate is

A Coordinate is a stable reference that points to one object inside a Project over time and without
ambiguity.

Project Context currently supports three coordinate classes:

1. **Project View object Coordinates**: the object type and stable `object_id` of a Project Profile,
   Goal, Role, Plan, Stage, Requirement, Issue, Work, or Resource;
2. **Project Document Coordinates**: a stable `document_id`;
3. **Meeting Coordinates**: a stable `meeting_id`.

A Coordinate is not:

- a title or name;
- the current body or summary;
- a particular Revision;
- one Nostr event;
- one Meeting Speech;
- the Agent Runtime that created or handled the object;
- a row in an embedding or vector index.

All of those can change without changing the Coordinate identity.

```text
Work Coordinate: { type: Work, id: W-17 }

revision 1  Design the Desktop entry point
revision 2  Add error-recovery states
revision 3  Complete interaction acceptance

All three state changes still refer to the same W-17.
```

A Coordinate must also belong to the current Project / Community. A bare URL, file path, chat
fragment, or model output does not automatically become a Project Context Coordinate merely because
it “looks relevant.” An external asset must first receive a project-local identity through a
Resource or Document. Any new coordinate type must likewise define stable identity, lifecycle,
permissions, and canonicalization rules first.

## 4. Coordinate context: understanding the object itself

“Coordinate context” is the name this document gives to a reading process:

> Starting from a stable Coordinate, resolve the object’s canonical content, current Revision,
> lifecycle, source evidence, and necessary direct references within the caller’s current
> authorization scope.

The content remains owned by its original source domain; it is not copied into Project Context:

- a Requirement provides what should be implemented or satisfied, its status, and its planning
  position;
- Work provides the task to perform, what it handles, responsibility, and current state;
- a Resource provides a project-local resource identity, while its Guide Document explains how to
  find and use it;
- a Document provides stable identity, a current immutable Revision, and historical Revisions;
- a Meeting provides stable meeting identity and permission-verified Board, Speech, and result
  entry points.

Coordinate context first answers:

1. Exactly which object am I handling?
2. What is its current authoritative content, source, and Revision?
3. After the object changes, can I still return to the same object?

Given a Work Coordinate, an Agent can first read back that Work, the Requirement or Issue it handles,
its responsible Role, and its direct references without loading the Project’s entire history into
every prompt.

### 4.1 Direct facts still belong to their source domains

Information that drives authorization, lifecycle, or automatic behavior must be written to an
explicit domain state. For example:

- which Plan a Stage belongs to;
- which Requirement or Issue a Work handles;
- whether a Work is complete;
- which Role is responsible for a Work;
- who currently holds a Role;
- which Guide Document a Resource uses.

These facts cannot exist only in a Context Document. Otherwise the Project acquires two competing
truths: machines read one state while Humans and Agents infer another from explanatory prose.

### 4.2 A Context Reference is not a Context Edge

Project View objects other than Resources can directly hold Context References to Resources or
Documents. A Resource itself can reference only Documents and uses its primary Guide Document for
usage instructions. These are direct reading entry points owned by one object.

A Project Context Edge is different: it connects two or more Coordinates and explains why that
entire set of objects must be understood together. Both structures can coexist, but neither is
automatically generated from or synchronized with the other.

## 5. Relational context: explaining what lies between objects

Some information does not belong to any single object. It is valid only when multiple objects are
considered together. For example:

- why two Requirements must be delivered together;
- why frontend Work is constrained by the protocol boundary of a Relay Resource;
- why a Meeting changed the implementation order of later Work;
- why a Document applies only to a particular combination of Stage, Role, and Resource.

Relational context has two parts:

```text
ProjectContextEdge
├── coordinates            exact, unordered Coordinate set; answers “what does this apply to?”
└── context_documents      one or more versioned Documents; answers “why?”
```

### 5.1 An Edge anchors only the exact scope

An Edge stores only one structural fact: **this set of Coordinates shares context that must be
understood together.**

```text
{Requirement A, Work B} == {Work B, Requirement A}
{Requirement A, Work B} != {Requirement A, Work B, Resource C}
```

An Edge is undirected. It does not assume relationship types such as `depends_on`, `causes`,
`blocks`, or `influences`.

Three or more Coordinates form a Hyperedge. A Hyperedge expresses a whole-set condition and is not
automatically decomposed into pairwise relationships:

```text
{A, B, C}

does not automatically produce {A, B}, {A, C}, or {B, C}.
```

This prevents an Agent from incorrectly applying an explanation that is valid only under the full
`{A,B,C}` condition to `{A,B}` alone.

### 5.2 Documents carry open-ended semantics

An Edge stores no prose body. When an ordinary Project Document is bound to an Edge, it takes on the
structural role of Context Document while continuing to reuse the Document model’s stable identity,
immutable Revisions, current version, author information, and tombstone rules.

A Context Document can explain:

- why the relationship exists and what evidence supports it;
- the direction, conditions, and strength of a dependency;
- the possible effect of changing one object;
- scope, exceptions, compatibility conditions, and risks;
- how work in the Project can verify or disprove the explanation.

There is only one Edge for one exact coordinate set, but that Edge can bind multiple Context
Documents. One can record historical reasons, another compatibility limits, and a third rollback
boundaries; there is no need to duplicate the same Edge three times.

A Document can belong to at most one active Edge as a Context Document at a time, preventing the
same explanation from acquiring two competing scopes. That Document can still appear on other Edges
as a Document Coordinate being explained.

### 5.3 The two structural roles of a Document

One Document can play two different roles in the model:

- when it appears in `coordinates`, it is one of the objects being explained;
- when it appears in `context_documents`, it carries the explanation for that Edge.

Binding a Document as a Context Document does not automatically add it to the coordinate set.
Using a Document as a Coordinate does not automatically make it an explanatory carrier.

## 6. Why Coordinates must come first

Here, “first” primarily means **prior in authoritative modeling and product design**.

### 6.1 Identity before reference

Titles, bodies, and keywords can all change or collide. A stable Coordinate separates “which object
is this?” from “what does the object currently say?” Later members can continue referring to the
same object instead of guessing again from text.

### 6.2 Facts before explanations

Project View first answers “what is the Project, what does it contain, and where is it now?” Project
Context then answers “why are these things related?” If current state cannot itself be read directly,
the relational explanation takes on responsibilities that do not belong to it and creates shadow
state.

### 6.3 Scope before semantics

A relationship explanation must first identify the exact objects to which it applies before it can
discuss reasons and effects. `{A,B}` and `{A,B,C}` are different semantic scopes; natural language
or similarity cannot automatically substitute one for the other.

### 6.4 Project and permission boundaries before relevance

The system must first verify:

- whether the objects belong to the same Project;
- whether the caller may discover and read them;
- whether references point to real canonical objects;
- whether each object is currently active, terminal, or tombstoned;
- which Revision a read or mutation is based on.

“Semantically related” does not expand read, write, Runtime, Sandbox, or external-system permissions.

### 6.5 Revision boundaries before automatic propagation

Separating Coordinates, Edges, and Documents lets the system handle three kinds of change
independently:

- object content changes: update the source object;
- relationship explanation changes: create a new Document Revision;
- relationship scope changes: explicitly detach / attach.

The system does not have to guess whether one text edit means rewriting object state, changing graph
topology, or deleting a historical relationship.

### 6.6 Readable sources before derived results

Titles, summaries, bodies, and states owned by source objects remain canonical content. A summary can
serve as a retrieval hint, but it does not replace the object’s full content. Embeddings, semantic
paths, UI caches, and summaries separately generated by clients or models are derived reads. They can
help discover content, but the final result must return to the canonical object, Revision, or
Document identified by the Coordinate. An embedding, cache, or model output cannot become the fact
source.

### 6.7 Discoverable entry points before full injection

An Agent’s work normally already has a starting point: the current Role, Work, Requirement,
Document, or Meeting. Stable Coordinates let the Agent expand context on demand instead of injecting
the full Project history:

```text
Current Work Coordinate
  → read back the Work's canonical content
  → incident(Work) discovers directly related Edges
  → inspect each Edge's exact Coordinate set and Document metadata
  → read only the Context Document needed for the current problem
  → continue discovery through related Coordinates if necessary
```

## 7. What “Coordinates first” does not mean

### 7.1 It is not the chronological order of real-world discovery

A Human or Agent can discover a piece of context in practice before realizing that the Project is
missing a Requirement, Issue, or Work. That insight can lead to creating a new object.

“Coordinates first” means only that, when a relationship enters canonical project state, its actual
participants must first have stable identities and only then be connected by an Edge. Do not invent
fake objects merely to satisfy the Edge shape. An explanation about only one object should not create
a self-edge either.

### 7.2 It does not require knowing a Coordinate before querying

Graph semantic queries can provide only a natural-language problem. Both `initial_coordinates` and
`context_coordinates` are optional:

- `initial_coordinates` are explicit traversal starting points;
- `context_coordinates` are a soft semantic environment that influences recall and ranking;
- a problem-only query can discover candidate roots before the caller knows any Coordinates.

“Coordinates first” is therefore a condition for persistent relationships, not a condition for a
user to begin retrieval.

### 7.3 It does not mean a Coordinate remains valid forever

A stable Coordinate guarantees continuity of identity, not that the object’s meaning never changes
or that the object remains active. Readers must also inspect the current Revision, lifecycle, and
source evidence.

## 8. Why not define a type and state machine for every relationship

More structured relationships are not always better.

Relationships that machines must execute should become explicit domain models. A Stage must belong
to a Plan; a Work must handle a Requirement or Issue; Assignments and Commitments have explicit
authorization and lifecycles. These relationships drive state and behavior, so types and state
machines are worthwhile.

Explanatory semantics across objects are open-ended. The same object set can simultaneously carry
historical reasons, implementation dependencies, organizational constraints, compatibility risks,
temporary exceptions, and experiential judgments. Continually adding relationship types such as
`depends_on`, `derived_from`, `compatible_with`, and `supersedes_when` would create:

- an ever-growing relationship vocabulary that is hard to stabilize;
- direction, cardinality, lifecycle, and migration rules for every relationship;
- complex meanings compressed into labels that look precise but are actually ambiguous;
- Agents inventing certainty to satisfy a schema;
- a requirement to change code and databases before writing back any new semantic distinction.

Carryforth therefore separates two kinds of information:

- **structural facts that machines must judge strictly** enter explicit domain models;
- **open-ended, explanatory, second-order semantics that require human language** are carried by an
  exact Edge scope and versioned Documents.

This is not a rejection of structure. It applies structure where stable execution requires it. If a
relationship later needs to drive authorization or automatic state transitions, it can be promoted
to an independent domain contract based on demonstrated needs. Until then, Carryforth does not try
to enumerate open semantics in advance.

The cost must also remain explicit: the system does not automatically verify the causality,
direction, or truth claimed in a Context Document. Humans and Agents participating in the Project
must continue to verify and revise those explanations.

## 9. Frontend and backend context example

Suppose the same Requirement produces both frontend and backend Work:

```text
Requirement R: User-controlled Agent recall

Work F: Desktop recall configuration and explanation UI
Work B: Relay recall, authorization, and result signing
Role F: Frontend
Role B: Backend
Resource UI: Desktop interaction specification
Resource API: Relay query contract
```

The Project can preserve two different relational contexts:

```text
Edge {R, Work F, Role F, Resource UI}
└── Document: frontend presentation, input state, error recovery, and explainability boundaries

Edge {R, Work B, Role B, Resource API}
└── Document: Provider egress, NIP-98 authorization, traversal budgets, and signing boundaries
```

Both Edges include Requirement R, but their scopes differ.

An exact `incident` query from Work F deterministically discovers the frontend-local relationship;
starting from Work B discovers the backend-local relationship. This difference comes from explicit
Coordinates and Edges, not from model inference.

An optional graph semantic query can further use Role, Work, and other Coordinates as the query
environment to influence recall and ranking. The following distinctions must remain clear:

- exact graph queries guarantee Edges that satisfy the coordinate-set condition;
- semantic queries provide derived paths with source evidence, but do not guarantee a unique answer
  matching human expectations for every problem;
- a context Coordinate is not an ACL, hard filter, mandatory starting point, or new persistent Edge;
- similarity, conditioned score, and environment gain are not fact confidence, causal proof, or
  project priority;
- a Relay signature proves result provenance and request binding, not that the semantic conclusion
  is inherently correct or complete.

## 10. How Agents read and maintain context

### 10.1 Reading

1. Confirm the current Project and caller identity.
2. Obtain a stable Coordinate from the current Role, Work, Requirement, Document, or Meeting.
3. Read back the Coordinate’s current canonical content, Revision, and lifecycle.
4. Use `exact`, `incident`, or `contains-all` to discover explicit relationships.
5. First inspect the Edge’s exact Coordinate set and Context Document metadata.
6. Read only the bodies needed for the current task, preserving source evidence.
7. When more open-ended discovery is needed, use the gated semantic path query.
8. Perform canonical readback for Coordinates returned by semantic results instead of treating the
   derived result itself as fact.

### 10.2 Writing

1. First determine whether the new information belongs to an explicit domain field or state.
2. If it does, update the corresponding canonical Requirement, Issue, Work, Role, Document, or other
   object.
3. If it is valid only when several objects appear together, choose the real, exact Coordinate set.
4. Query whether an Edge already exists for that set.
5. Create or revise an ordinary Project Document that states the reasons, effects, evidence, and
   boundaries.
6. Explicitly attach it to the exact Coordinate set and read back the canonical Edge.
7. Do not automatically create relationships from chat, a Meeting Board, tool success, or model
   inference.

This is what “Coordinates first” means as a writing discipline: first write facts back to the object
that truly owns them, then establish a relationship for cross-object explanations. A Context
Document cannot substitute for a Requirement, Issue, Work, or other domain state that ought to
exist.

## 11. Lifecycle and history

| Change | Result |
|---|---|
| Object content changes | Coordinate identity remains stable; read the new canonical state or Revision |
| Context Document content changes | Creates a new Document Revision; the Edge Coordinate set remains unchanged |
| Relationship scope changes | Explicit detach / attach; the old scope is not silently reinterpreted as the new one |
| Coordinate is tombstoned | Existing Edge retains the stable Coordinate and presents its lifecycle state |
| Context Document tombstone requested | An active binding blocks the operation; detach it from the Edge first |
| Last Context Document is detached | The empty Edge disappears; the Document itself is not deleted |

When a relationship is created, its Coordinates and Context Document must pass current Project and
lifecycle validation. A Meeting must also be in a currently attachable phase. Once the relationship
exists, tombstoning a Coordinate does not silently shrink the Edge and does not cascade-delete its
explanatory Documents.

Later readers can therefore see not only “what is believed now,” but also which objects an
explanation once applied to and why object deletion did not silently rewrite historical
relationships.

## 12. Three easily confused forms of Context

| Concept | Persistent? | Purpose |
|---|---:|---|
| Project View Context Reference | Yes | Direct reference from one Project View object to a Resource or Document |
| Project Context Edge | Yes | Preserves the exact, explicit relationship scope between two or more Coordinates |
| Semantic-query `context_coordinates` | No | Soft recall and ranking environment for one query |

Semantic-query paths are also derived reads, not new persistent relationships. Only Project View,
Documents, Context Edges, or other domain state explicitly written and validated by the Relay become
canonical Project facts.

## 13. Design boundaries

This model intentionally does not promise to:

- infer Edges automatically from messages, Documents, or object state;
- replace Project View’s existing strongly typed relationships with Edges;
- enumerate relationship types for open semantics in advance;
- automatically inject Context Documents into every Agent turn;
- let semantic similarity determine facts, permissions, responsibility, or action;
- let a Coordinate in one Project cross into another Project;
- automatically rewrite historical relationships when an object is tombstoned;
- guarantee that a stored explanation is inherently complete, current, conflict-free, or correct.

The system guarantees stable identity, exact scope, canonical writes, Revisions, lifecycle, and
permission boundaries. Humans and Agents participating in the Project must still verify, revise, and
maintain whether the context is accurate and useful.

## 14. Derived design principles

1. **Identify objects before explaining relationships.** Without stable participants, project
   context cannot be maintained.
2. **Separate identity from content.** Content can evolve; Coordinates must remain referenceable.
3. **Separate facts from explanations.** Direct state returns to its source domain; cross-object
   meaning enters Context.
4. **Separate scope from semantics.** An Edge defines the exact applicable scope; a Document carries
   the open explanation.
5. **Separate strongly typed state from open semantics.** Relationships that machines must execute
   enter domain models; the rest are not forced into code.
6. **Prefer explicit relationships to implicit inference.** Semantic retrieval supports discovery;
   it does not replace canonical facts.
7. **Prefer on-demand reads to full injection.** An Agent progressively discovers context from its
   current work Coordinate.
8. **Prefer revision to silent overwrite.** Changes to objects, explanations, and relationship scope
   each leave verifiable records.
9. **Relevance grants no permission.** Every read, write, and query remains subject to Project /
   Community boundaries.

“Coordinates first, context second” means first giving things in a Project a stable, verifiable, and
readable existence, and then allowing Humans and Agents to accumulate relationships and explanations
around them over time. The result is not a pile of text that later members must guess at again, but a
project knowledge network that can evolve with the Project without losing boundaries, sources, or
history.

## Further reading

- [Carryforth Core Model](../core-model.md)
- [Core Design: Role Continuity](role-continuity.md)
- [Core Design: Context-Aware Semantic Graph Retrieval](context-aware-semantic-graph-retrieval.md)
- [Core Design: Meeting](meeting.md)
- [Project View definition](../../stage/project-view/project-view.md)
- [Project Document](../../stage/document/document.md)
- [Project Context domain specification](../../stage/project-context/project-context.md)
- [Project Context Desktop design](../../stage/project-context/desktop-spec.md)
- [Project Space Constitution](../project-space-constitution.md)
- [Current Status and Capability Boundaries](../current-status.md)
