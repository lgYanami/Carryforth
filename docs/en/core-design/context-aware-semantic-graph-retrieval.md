# Context-Aware Project Context Semantic Graph Retrieval

> This document explains a core Carryforth design: how the same question, viewed in different
> contexts such as a Role or Work, can yield different yet still relevant and traceable context
> paths from the same Project-owned context graph.
>
> This document describes the product mental model. It does not redefine the query protocol,
> scoring constants, database structure, or Provider operations contract. For exact implementation
> boundaries, see the
> [staged Project Context semantic graph query implementation plan](../../stage/semantic/project-context-graph-semantic-query-implementation-plan.md).

## 1. Core judgment

> Context-graph retrieval is fundamentally the selection of different project reading paths for the
> same question according to the current context environment, thereby obtaining different but
> relevant context. It does not preserve separate versions of project knowledge for different
> Agents.

Project Context is a context graph shared and owned by the Project. Environment Coordinates such as
Role and Work are soft lenses through which to view that graph. They can change which content is
worth reading first and which real relationships are worth expanding, but they do not partition
project context, expand permissions, or create a private graph owned by an Agent.

```text
                    The same Project Context graph
                                │
                         The same question Q
                                │
                 ┌──────────────┴──────────────┐
                 │                             │
          Frontend Role / Work          Backend Role / Work
          context environment E1        context environment E2
                 │                             │
          context path P1                context path P2
                 │                             │
          frontend context C1            backend context C2
```

The design aims for `P1 != P2` and `C1 != C2`, provided that the Project actually contains objects,
relationship explanations, and semantic evidence capable of distinguishing the two environments.
The system does not fabricate paths merely to produce a difference. If the same path is the most
relevant in both environments, or if existing project content cannot express a distinction, the two
queries may return the same result.

## 2. First distinguish three concepts

“Context,” “context environment,” and “context path” are related, but they are not the same thing.

### 2.1 What context is

Here, context is the project content a Human or Agent needs to load into a limited context window in
order to understand the current problem and continue acting.

It combines two kinds of content:

1. **Coordinate context**: the current project content identified by stable Coordinates such as
   Roles, Work, Requirements, Issues, Resources, Documents, and Meetings. It answers “what is this
   object, and what state is it in now?”
2. **Relational context**: the reasons, dependencies, effects, exceptions, and boundaries preserved
   by real Edges / Hyperedges and their Context Documents. It answers “why must these objects be
   understood together?”

```text
Coordinate ──return to source domain──> current object content
     │
     └──enter a real Hyperedge────────> Context Document ──> another related Coordinate
```

Context is therefore neither one large text block detached from project objects nor a collection of
“similar content” in a vector database. Context that can ultimately support action must return to
stable Project objects, current Revisions, and explicit relationship scopes.

For why both kinds of context begin with stable Coordinates, see
[Core Design: Coordinates First, Context Second](coordinate-and-context.md).

### 2.2 What a context environment is

A context environment describes:

> Where in the Project the Human or Agent stands when asking this question, which responsibility it
> currently holds, and which work it is handling.

It can be expressed by one or more stable Coordinates, for example:

- the current Role;
- the current Work;
- the Requirement or Issue currently being addressed;
- the Document currently being consulted;
- the collaborative setting formed by a Meeting.

The context environment belongs to **this query**, not to the Agent’s identity. The same Agent can
switch between environments, and different Agents can view the Project from the same Role or Work
environment.

Environment Coordinates declare only a soft lens for the current retrieval. An environment is not:

- the name of an Agent’s private knowledge base;
- a new persistent Project Context object;
- a mandatory retrieval starting point;
- the only subgraph the caller may access;
- an ACL, membership status, or action authority;
- an automatic expansion of a Role’s Assignment, all Work, graph neighborhood, and Runtime memory.

The current implementation constructs a bounded query signal from the environment Coordinate’s
current canonical overview. It does not inject the object’s complete body, every adjacent object, or
the Agent’s private Runtime Context into the Provider.

### 2.3 What a context path is

A context path is a traceable reading route through the unified Project Context graph. It can begin
at:

- a Coordinate recalled semantically by the problem;
- a Context Document recalled semantically by the problem;
- an initial Coordinate explicitly specified by the caller.

The path then expands only along real relationships already present in the Project:

```text
Coordinate root
  └── current incident Edge / Hyperedge
        ├── Context Document: why this Coordinate set is related
        └── complete Coordinate set
              └── related Coordinate

Context Document root
  └── its current active binding
        └── exact Edge / Hyperedge
              └── related Coordinate
```

A context path answers two questions at once:

1. Which project objects are worth reading next?
2. Why is it valid to travel from the current object to those objects?

A path is not a new project fact or a copied body of context. It is navigation to context and
evidence for the relationships followed:

```text
The context environment determines the lens through which to observe
The context path determines which real relationships to follow while reading
Coordinate content and relational Documents form the context ultimately loaded into the window
```

## 3. Why there is always one unified graph

### 3.1 Agent context must be externalized, not privatized

An Agent has a limited context window, and Sessions and Runtimes can end, be compacted, or be
replaced. Long-term project context therefore cannot exist only inside one model process’s window;
it must be externalized into the Project.

But “externalized” does not mean “a private context for every Agent.” The real problem is:

> Which Project content should be selected and loaded this time?

It is not:

> Which Project content belongs to this Agent?

Creating private context for each Agent, Role, or Runtime first isolates context that was originally
shared by the Project, then adds another abstraction to manage that isolation. This sidesteps the
retrieval problem without solving it.

The system would then have to manage:

- copying and promotion between public and private context;
- synchronization, merging, and conflict resolution across private spaces;
- migration when an Assignment, Role, Session, or Runtime changes;
- which space contains the current version of a project relationship;
- which context space an Agent should query before every action.

These mechanisms increase both system maintenance cost and Agent cognitive load. More importantly,
they hide real cross-Role project constraints behind artificial isolation boundaries.

### 3.2 What is unified is project identity, provenance, and relationships

Carryforth therefore maintains only one Project-owned Context Graph:

- Requirements, Issues, Work, Resources, Documents, Meetings, and other objects each retain one
  stable identity;
- object content continues to be governed by its source domain and Revisions;
- cross-object semantics continue to be carried by exact Edges / Hyperedges and versioned Context
  Documents;
- different queries select different paths through different environment lenses instead of copying
  objects or relationships.

A unified graph does not mean injecting all project content into every Prompt, nor does it mean that
every member can read everything unconditionally. Queries remain constrained by Project /
Community, caller identity, source domain, lifecycle, and capability gates. They progressively
expose only content that is authorized and genuinely needed for the current query.

An Agent can still have local temporary state, drafts, or Runtime Memory. Those are working
materials, not authoritative carriers of Project continuity. Content that affects other members,
later responsibility, or future work must be explicitly written back to shared Project View,
Document, Context, Meeting, Checkpoint, or other canonical objects.

### 3.3 Isolate the observation lens, not project context

The design choice can be summarized as:

> Do not isolate project context for each Agent; specify a context environment for each query.

Frontend and backend receive different content not because each owns separate knowledge, but because
they select different reading routes through the same project knowledge from different observation
lenses.

## 4. Problem, starting point, and environment are orthogonal inputs

One semantic graph query can include three input classes:

| Input | Question answered | Semantics |
|---|---|---|
| `problem` | “What am I trying to solve now?” | Required; always dominates recall |
| `initial_coordinates` | “Where do I explicitly want to start?” | Optional structural starting points |
| `context_coordinates` | “From what environment am I observing?” | Optional soft recall and ranking lens |

These inputs cannot substitute for one another.

### 4.1 The Problem determines what to seek

The natural-language problem is the primary signal. A problem-only query with no initial or context
Coordinate is a valid entry point, suitable for discovering candidate roots when the caller does not
yet know which Coordinates exist in the graph.

Changing the context environment should not rewrite the problem into another problem. Whether a
frontend or backend Role is used, “How are recall and the user-control experience currently
designed?” remains the same question.

### 4.2 An Initial Coordinate determines where traversal begins

An initial Coordinate is a structural root explicitly selected by the caller. It expresses “start
from this Work or Role.”

It is not synonymous with context environment, and it does not constrain global candidate recall to
a private subgraph of that Coordinate. One Coordinate can be both initial and context: the former
declares the traversal starting point; the latter declares the observation lens.

### 4.3 A Context Coordinate determines what is more worth reading first

A context Coordinate participates in relevance judgment, but does not automatically become a root,
hard filter, or mandatory waypoint.

When the product needs a deterministic check of a Role’s or Work’s direct relationships, use an
initial Coordinate or an exact `incident` / `contains-all` structural query. Do not delegate a
deterministic structural requirement to a soft semantic lens.

## 5. How the environment affects results without swallowing the problem

### 5.1 Neutral and Conditioned observations

The current implementation first forms a problem-only neutral query, then a conditioned query for
each environment Coordinate:

```text
Q0 = problem
Qi = problem + current canonical overview of context_coordinate_i
```

Coordinate and Context Document candidates in the unified semantic index independently receive:

- problem relevance to `Q0`;
- conditioned relevance to each `Qi`;
- positive environment gain of conditioned relevance over problem relevance.

The environment contributes only the increment “because the caller stands here, this candidate
deserves additional attention.” It cannot use an environment-similar item with low problem
relevance to completely rewrite the question, nor can it present negative gain as environment
evidence.

### 5.2 Environment influence is bounded

The current scoring structure keeps the problem signal dominant, gives the environment only a
bounded share, and gives an explicit initial anchor an even smaller structural contribution. Across
multiple environments, only the strongest and a small portion of the second-strongest continue to
contribute, preventing a large collection of context Coordinates from drowning out the problem.

Automatic roots also retain two neutral protections:

- the strongest problem-only root is preserved;
- at least part of the root quota is reserved for neutral candidates.

These limits allow the environment to change the candidate set, ranking, and final paths without
creating a retrieval tunnel in which the system may “search only inside one Role’s world.”

### 5.3 Do not force differences

“Different context environments yield different context paths” is a capability this design seeks,
not a requirement to manufacture differences for every input.

Two queries can return the same path for common reasons, including:

- that path genuinely is the most relevant shared context in both environments;
- the Role or Work overview lacks enough distinguishing information;
- the corresponding Work, Document, or Edge has not been explicitly established in the Project;
- a relevant source does not yet have a current semantic head;
- a candidate received environment gain but still did not fit within the root or path budget;
- the relationship explanation does not contain enough evidence for a finer semantic distinction.

The system cannot fabricate relationships, relax permissions, or ignore problem relevance merely to
satisfy “the results must differ.” The correct remediation is to improve project modeling, semantic
inputs, candidate recall, and bounded ranking—not to split the context graph.

## 6. Semantics only select paths; the real graph determines where they can go

Semantic similarity only selects and ranks existing candidates. It cannot create adjacency.

One Coordinate hop has this structure:

```text
Current Coordinate U
  → a real undirected Hyperedge E containing U
  → one Context Document D currently bound to E
  → another Coordinate V in E's complete Coordinate set
```

Each Context Document independently contributes one relationship semantic. The system first judges
which relationship explanation is more relevant to the problem and environment, then selects the
next Coordinate from the real members of that complete Hyperedge.

The following boundaries must hold:

- Edges / Hyperedges are undirected;
- `U → E → V` is only traversal order for this query, not a causal, dependency, or temporal direction
  in the domain;
- `{A, B, C}` is an exact three-party relationship scope and does not automatically produce
  `{A, B}`, `{A, C}`, or `{B, C}`;
- returning one relation Document does not mean that it summarizes every meaning carried by the
  Edge;
- lifecycle and readiness can prevent further expansion of a target, but cannot remove members from
  the complete Edge identity in the result;
- a query never creates, completes, splits, or modifies an Edge.

Semantic paths therefore retain explicit relationship grounds rather than collapsing into “these
two text vectors look similar.”

## 7. How a traceable result becomes usable context

### 7.1 What each path preserves

The returned result preserves the structure and source evidence needed to understand and verify a
path, including:

- the root’s source type and stable identity;
- each hop’s Edge key and complete Coordinate set;
- the Context Documents currently bound to the Edge;
- the selected relation Document and exact binding for this hop;
- source Revision / change basis for Coordinates, Documents, and Meetings;
- semantic generation, snapshot, and scoring explanation;
- coverage, stopping, and omission reasons.

This lets callers distinguish what the Project explicitly preserved as a relationship from what was
only selected and ranked for this query.

### 7.2 Currentness is a query snapshot, not eternal freshness

Recall, hydration, and graph traversal complete within one consistent Stage C database snapshot.
The result proves that “within this snapshot, these sources and relationships had these Revisions
and currentness evidence.” It does not prove that nothing changed before the response arrived.

If an object is updated after the query, the caller must compare the result evidence with current
canonical readback instead of treating the old path as permanently frozen project state.

### 7.3 Signed results and canonical readback

The Relay signs the result Event and binds it to the current Project, caller, and exact request body.
This proves:

> This Relay returned this snapshot-derived result for this request.

It does not prove that relevance is inherently correct, that a relationship Document is true, that
the result exhausts every possible or relevant path, or that the Project should act on the result.
The exact Edge, complete Coordinate set, binding, continuity, and request budget of returned hops are
still verified by the closed result contract and SDK.

After verifying the signed result, `cf` separately derives unsigned but canonicalized
`read_commands` from its stable identities. Callers can use those commands to read the current
authoritative content of Project View objects, Documents, and Meetings. The commands read current
state at execution time, which may already be newer than the query snapshot. They are neither part
of the signed result nor an exact replay of the query snapshot.

Context ultimately loaded into an Agent window should come from those canonical objects and
relationship evidence, not from vector previews or scores alone.

## 8. A frontend and backend environment example

Suppose a Project contains one authorization Issue and two real relationships:

```text
Edge F = {
  Authorization Issue,
  Frontend Role,
  Desktop Work,
  Frontend interaction Document
}

Edge B = {
  Authorization Issue,
  Backend Role,
  Relay Work,
  Backend authorization Document
}
```

Each Edge binds a Context Document explaining why its Coordinates must be understood together.

For the same question, “How are recall and the user-control experience currently designed?”:

- a problem-only query should first discover the most relevant overall entry points in the Project;
- the Frontend Role / Desktop Work environment should increase the chance that frontend objects and
  relationship explanations become roots or path material;
- the Backend Role / Relay Work environment should increase the chance of backend objects and
  relationship explanations;
- both results can still share the authorization Issue, a common Requirement, or cross-stack
  constraints because they come from the same Project;
- every path must follow the real complete set of `Edge F` or `Edge B`, rather than creating a
  temporary edge from the words “frontend” or “backend.”

The expected outcome is not two unrelated answers, but **different yet related** project context:
the shared problem remains visible, while current responsibility and work environment determine
which relationships deserve earlier expansion.

If traversal must deterministically start from `Desktop Work`, provide it as an initial Coordinate,
either alone or also as context. If it should only influence priority, provide it as a context
Coordinate.

## 9. How an Agent should use this capability

A typical workflow is:

1. State the current natural-language problem clearly.
2. Select the context environment from a verified current Role, Work, or other project object.
3. If traversal must begin from a known object, additionally provide an initial Coordinate.
4. Run the query and inspect neutral and conditioned evidence, coverage, and stopping reasons.
5. Perform canonical readback of stable objects along the returned paths.
6. Assemble the actual working window from current object content and Context Documents.
7. If the Project lacks a real relationship, use ordinary domain operations to explicitly create or
   revise a Document / Edge.
8. Do not write the retrieval path, score, or model judgment directly into the Project as fact.

A Managed Agent’s Harness should not infer the current Role or Work merely from process identity.
The higher-level caller should obtain suitable environment Coordinates from current Project state
and pass them explicitly. Incorrect or stale environments gain no additional permissions.

## 10. No rewrite of canonical Project facts or relationships

A semantic graph query is a derived read over canonical Project state. It does not:

- create, update, or delete a Project Context Edge;
- create, revise, or tombstone a Project Document;
- modify Project View, Meeting, Role, Assignment, Work, or Commitment;
- persist the problem, query vector, or retrieval path;
- promote similarity into a relationship, fact, responsibility, or permission;
- make a Role or Agent the owner of context.

Embeddings and semantic generations are deletable, rebuildable derived indexes, not new Project fact
sources. Provider admission, rate-limit quotas, and operational metrics can update derived
operational state. Those records contain no problem body and do not constitute persistent writeback
to Project View, Document, Context, Meeting, or a retrieval path.

Permission validation is independent of relevance scoring. Community membership, caller identity,
source visibility, query gate, lifecycle, and currentness continue to apply at the corresponding
boundaries for Provider egress, candidate recall, and result release. Environment Coordinates,
graph adjacency, similarity matches, and Relay signatures cannot expand read, write, Runtime,
Sandbox, Secret, or external-system permissions.

## 11. Current implementation and qualification boundaries

The current code implements:

- problem-only, explicit initial, and context-lens inputs;
- independent conditioned evidence for each context Coordinate;
- candidate scoring dominated by the problem with bounded environment influence;
- preservation of neutral roots;
- multi-hop traversal along real undirected Hyperedges;
- complete Edge / binding / source / semantic provenance;
- Relay-signed exact request binding;
- `cf` verification and canonical-readback navigation;
- queries that do not modify any Project relationship.

But “the mechanism is implemented” does not mean “the relevance objective is fully qualified.”
Existing acceptance evidence has shown that a Backend Role / Work environment can promote backend
Work and its corresponding Edge, and that a frontend environment can produce conditioned gain. It
has not yet shown that every meaningfully distinct environment, including frontend and backend,
consistently returns the different paths a Human expects within default root / path budgets.

The accurate current conclusion is therefore:

> The context-environment lens over the unified graph can already influence recall and ranking;
> “different environments consistently yield semantically correct different paths” remains a
> product objective requiring further calibration and acceptance.

This capability also requires a Provider, semantic index, Community index/query gates, and
confirmation before problem data leaves the system. Relevance, resource isolation, long-running
operation, and production deployment remain under qualification. It must not be described as
production-ready, and environment gain must not be interpreted as fact confidence, causal proof, or
project priority.

## 12. Non-goals

This design does not attempt to:

- create a private knowledge graph for each Agent, Role, or Work;
- guarantee that changing the environment always yields different, disjoint, unique, or complete
  results;
- make a context Coordinate an ACL, hard filter, or automatic root;
- use vector similarity to discover and persist Project Context Edges automatically;
- decompose a Hyperedge into implied binary relationships;
- judge Context Document content to be inherently correct, sufficient, conflict-free, or current;
- use a Relay signature to endorse semantic relevance;
- treat a retrieval path as new long-term Agent Memory;
- replace explicit maintenance of Project Context by Humans and Agents.

## 13. Derived design principles

1. **The Project owns context; Agents read it on demand.** Continuity does not depend on one window,
   Session, or Runtime.
2. **Externalization does not mean privatization.** Limited windows require retrieval, not an
   artificial partition of project knowledge.
3. **The environment belongs to the query, not the identity.** Role / Work is an observation
   position, not a context owner.
4. **The problem always dominates.** The environment contributes only explainable, bounded marginal
   influence.
5. **Different paths come from real differences.** Do not fabricate relationships or sacrifice the
   shared problem semantics merely to produce different results.
6. **Semantics select paths; graph structure constrains paths.** Traverse only real, complete,
   undirected Hyperedges.
7. **Paths must be traceable.** Every step preserves Coordinate, Edge, Document, Revision, and source
   grounds.
8. **Derived reads must return to canonical facts.** Signed results and scores do not replace
   canonical readback.
9. **Retrieval never writes back implicitly.** Only explicit, authorized ordinary domain operations
   can change the Project.
10. **Relevance grants no permission.** Environment, similarity, adjacency, and signatures cannot
    expand authority.

Context-aware semantic graph retrieval ultimately addresses this problem:

> When a Human or Agent can work only within a limited window, how can it use the current Role, Work,
> and problem to select a grounded reading route through the Project’s shared context graph and
> obtain context appropriate for the current action, without splitting the Project into drifting
> private memory spaces?

## Further reading

- [Carryforth Core Model](../core-model.md)
- [Core Design: Role Continuity](role-continuity.md)
- [Core Design: Coordinates First, Context Second](coordinate-and-context.md)
- [Core Design: Meeting](meeting.md)
- [Project Context domain specification](../../stage/project-context/project-context.md)
- [Project Context semantic graph foundation specification](../../stage/semantic/project-context-graph-semantic-foundation-spec.md)
- [Project Context semantic graph query implementation plan](../../stage/semantic/project-context-graph-semantic-query-implementation-plan.md)
- [Project Context Desktop semantic graph query qualification record](../../stage/semantic/desktop/project-context-semantic-query-desktop-qualification.md)
- [Semantic pgvector operations](../../semantic-pgvector-operations.md)
- [Current Status and Capability Boundaries](../current-status.md)
