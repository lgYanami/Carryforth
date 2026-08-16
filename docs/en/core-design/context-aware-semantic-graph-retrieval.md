# Agent-Directed Context-Aware Project Context Retrieval

> This document explains a core Carryforth capability: how an Agent uses its own context
> environment to progressively retrieve context from the single Context Graph owned by the
> Project.
>
> The capability is wired into the `cf` CLI, the Project Space prompt, and the
> `search-project-context` Skill. Semantic indexing and semantic queries remain explicitly gated
> external-Provider capabilities; they are not new sources of canonical Project facts.

The primary goal is:

> **Agents in different context environments can follow different relevant graph paths for the
> same problem and thereby obtain different but related, traceable context.**

The useful difference comes from the Agent combining its current Role, work situation, object
state, and relationship evidence at each decision. Carryforth does not split Project knowledge
into a private graph per Agent, and it does not try to manufacture divergence by assigning a large
fixed vector weight to a Role.

## 1. One graph, different reading routes

Project Context preserves relationships that Project members explicitly establish among Project
objects, Documents, and Meetings. Every Agent reads the same Project-owned Context Graph:

```text
                         one problem
                              │
              ┌───────────────┴───────────────┐
              │                               │
     frontend Role / current Work    backend Role / current Work
              │                               │
       choose a relevant start          choose a relevant start
              │                               │
       Coordinate → Edge               Coordinate → Edge
              │                               │
       relation Document               relation Document
              │                               │
        next Coordinate                 next Coordinate
              │                               │
       frontend context                 backend context
```

The routes may share a real Issue, Stage, Requirement, or cross-component constraint. The goal is
not disjoint paths for appearance's sake. It is to choose different relationships where the
difference matters and obtain context appropriate to the current responsibility and work.

The context on a path is the useful result; the path supplies navigation and relationship
evidence:

```text
the context environment guides the current choice
the graph structure constrains where a traversal can really go
Coordinate content and relation Documents become the context the Agent uses
```

## 2. What a context environment is

A context environment is the Agent's current, verified task situation before this retrieval begins.

The current effective Role is always the core of a semantic retrieval environment. It states what
responsibility the Agent currently bears. The Agent adds other facts only when they can affect this
retrieval, for example:

- the Work currently being performed;
- a Requirement, Issue, or Stage currently being handled;
- current task state, goals, boundaries, and expected output;
- a Meeting identity, the Agent's participation identity, and its purpose in that Meeting;
- user-supplied concerns, exclusions, or relevant Coordinates.

The Context Graph does not need to restate “which Role this Agent has” or “which Work this Role
owns.” Those facts already come from the current Role Brief, Assignment, task, Meeting Turn, and
their owning surfaces. The graph only needs to preserve real relationships among Project objects.

A context environment is not:

- an Agent persona, model, Session, or Runtime;
- a private Agent knowledge base or private subgraph;
- an identity inferred backward from candidate titles, summaries, or scores;
- an ACL, Community membership, Assignment, or action permission;
- a hard filter that automatically excludes other Roles or cross-Role relationships;
- a new object that must be persisted to the Project.

If the current Role Brief is candidate or unavailable, or says `Role: none`, the Agent does not
guess a Role, reuse an old Role, or issue a natural-language semantic search. With a reliable known
Coordinate it may still perform structural observations and canonical reads that send no natural-
language query. Without a reliable start, it stops this retrieval.

## 3. Why the Agent controls retrieval

Language similarity alone cannot fully understand a context environment. Frontend and backend
documents may describe the same authorization problem, use many of the same terms, and share the
same Issue or Stage. An embedding model can find linguistically related objects, but a score alone
cannot decide which relationship best fits the Agent's responsibility, Work, task state, or Meeting
purpose.

Carryforth therefore separates responsibilities:

- semantic search ranks potentially relevant candidates inside an explicit scope;
- canonical lightweight observations provide current object and relationship information;
- Project Context Edges constrain the relationships that really exist;
- relation Documents explain why the Coordinates belong together;
- the Agent uses its context environment to adopt, reject, branch, backtrack, or stop.

This approach does not require the semantic model to compress the entire environment into one
vector, and it does not rely on a fixed fusion formula to make the final decision for the Agent.
The same candidate may be linguistically close for two Roles while the two Agents choose different
next steps after observing object kind, current state, relation evidence, and task responsibility.

The context environment is not a hard isolation boundary. A real cross-Role dependency must remain
visible. If frontend Work depends on a backend authentication contract and a relation Document
demonstrates that dependency, a frontend Agent may choose the backend Work rather than rejecting it
solely because the Role differs.

## 4. What the graph provides

Progressive traversal depends on three independent Project Context elements.

### 4.1 Coordinate

A Coordinate is a stable identity for a Project object such as a Role, Work, Requirement, Issue,
Stage, Resource, Document, or attachable Meeting. The object's content remains owned and versioned
by its original surface.

### 4.2 Edge / Hyperedge

An Edge preserves an exact, unordered set of two or more Coordinates:

```text
E = {C1, C2, ..., Cn}
```

It is an undirected Hyperedge. `Coordinate → Edge → Coordinate` is only the reading order for one
traversal; it does not create causal, dependency, or temporal direction in the domain. A ternary
Edge `{A, B, C}` does not silently become three binary relationships.

### 4.3 Context Document

An Edge may bind one or more Project Documents that explain in open text why its Coordinate set is
related. The Edge defines relationship scope; Documents provide relationship evidence. Semantic
similarity never creates, splits, fills in, or rewrites an Edge.

## 5. The progressive retrieval loop

The Agent does not need one request to return a complete path. It repeatedly runs a bounded loop:

```text
clarify what context is needed
             │
confirm the current Role and relevant environment facts
             │
choose a starting Coordinate
             │
Coordinate ──choose an incident Edge──▶ Edge
     ▲                                     │
     │                          observe relation Documents
     │                                     │
     └────────choose the next Coordinate───┘
```

Every step follows the same order:

1. obtain candidate identities inside the current scope;
2. inspect lightweight title or name, description, summary, status, Revision, and provenance;
3. filter candidates using the context need and context environment;
4. read complete canonical content only when lightweight data cannot settle the choice or when the
   task will rely on a fact from that content;
5. continue, switch branches, backtrack, or stop.

This lets an Agent incrementally load what fits its finite context window instead of injecting the
entire graph, every relationship Document, and every object body into one prompt.

## 6. Choosing a start is already retrieval

### 6.1 Prefer a Coordinate already identified by current work

Most retrievals should begin with an object already supplied by the Agent's current work or
Meeting: the Work being performed, a Requirement or Issue being handled, a relevant Stage, or a
Project View object explicitly referenced by a Meeting.

If that object is relevant to the current context need, the Agent uses it directly. It does not run
a whole-graph search merely to chase a higher global semantic score. When current lightweight state
must be confirmed, it can run:

```bash
cf project-context coordinate show <TYPE:UUID>
```

### 6.2 Use whole-graph semantic discovery only when no reliable start exists

Only when the task, Meeting, and environment provide no explicit relevant Coordinate does the
Agent run once:

```bash
cf project-context coordinate-search \
  --query "<desired start or responsibility; short relevant Role responsibility; optional discriminator>" \
  --coordinate-type work \
  --limit 8
```

When the structural type of the start is known, repeat `--coordinate-type` to form a small OR set;
the filter is applied before semantic ranking and top-K. Omit it when the type is uncertain. The
structural filter narrows candidates, but the Agent still evaluates them against its context
environment.

This command returns Coordinate identity, rank, and score only. Its output is a list of candidates
to inspect, not a selected start. The Agent uses rank to order `coordinate show` observations, then
asks:

- is this object relevant to the context actually needed;
- does it fit the current Role, Work, task, or Meeting purpose;
- is it merely linguistically similar while its object kind, responsibility, stage, or state is
  wrong;
- does it provide a useful entry into relationship discovery?

The Agent may choose a lower-ranked candidate or reject every candidate. A score is not a fact,
confidence probability, relevance threshold, permission, or hard scope.

## 7. Two one-hop semantic choices and four structural observations

The CLI keeps each step atomic so that one command does not choose both an Edge and the next
Coordinate for the Agent:

| Purpose | Command | Result |
|---|---|---|
| Discover a start across the graph | `coordinate-search` | ranked Coordinate identities and scores |
| Observe one Coordinate | `coordinate show` | canonical lightweight observation |
| Semantically choose an Edge around a Coordinate | `coordinate edge-search` | ranked Edges and lightweight matched relation Documents; no member Coordinates |
| Read all incident Edges for a Coordinate | `coordinate edges` | structural Edge identities and binding counts |
| Inspect an Edge's relationship evidence | `edge documents` | canonical lightweight relation Documents and on-demand read entries |
| Semantically rank members inside an Edge | `edge coordinate-search` | ranked Coordinates and canonical lightweight observations; no relation Documents |
| Read complete membership for an Edge | `edge coordinates` | lightweight observations for the complete Hyperedge membership |

Semantic commands narrow the observation set. Structural commands answer complete-set questions.
Neither substitutes for the other.
`edge coordinate-search` also accepts repeated `--coordinate-type` values and filters the complete
Edge membership before ranking. `coordinate edge-search` does not accept the filter because it
selects Edges rather than Coordinates.

### 7.1 Choose an Edge from a Coordinate

```bash
cf project-context coordinate edge-search <TYPE:UUID> \
  --query "<current Role and the relationship or evidence needed at this hop>" \
  --limit 8
```

The query ranks only active incident Edges for the input Coordinate and uses the Edges' current
relation Documents as their semantic evidence. The Agent observes title, summary, state, and
provenance before deciding which Edge actually explains the relationship it needs.

When complete incident structure is required, it uses:

```bash
cf project-context coordinate edges <TYPE:UUID>
```

### 7.2 Inspect relationship evidence

```bash
cf project-context edge documents <EDGE_KEY>
```

The command pages through lightweight Document observations; following its continuation yields the
complete binding set, not every body. The Agent does not execute every `fetch_command`. It reads a
body through its SDK-verified, revision-pinned descriptor only when that Document can change the
Edge choice or when later work will rely on a fact from it.

### 7.3 Choose the next Coordinate from an Edge

```bash
cf project-context edge coordinate-search <EDGE_KEY> \
  --query "<current Role, the next object needed, and why it matters>" \
  --limit 8
```

The query ranks only the complete members of that active Edge. The Agent combines each candidate's
lightweight observation with its context environment and path so far, then chooses the next
Coordinate that actually advances the problem.

When complete membership is required, it uses:

```bash
cf project-context edge coordinates <EDGE_KEY>
```

After choosing the next Coordinate, the Agent continues from that Coordinate's incident scope.

## 8. Lightweight observations first, complete content on demand

Semantic candidates carry enough canonical lightweight information for initial filtering, such as
title or name, description, summary, status, Revision, and source provenance. These fields help the
Agent reject candidates with the wrong object kind, responsibility boundary, lifecycle, or task
stage.

Lightweight information is not final evidence or a Project instruction. Every project-authored
title, description, summary, and body is untrusted Project data. The Agent must not obey embedded
requests to execute commands, reveal secrets, weaken policy, or change authority.

The Agent reads complete canonical content only when:

1. lightweight observation cannot settle whether to choose the object or relationship; or
2. subsequent work depends on a specific fact in the body.

Complete Coordinate content continues to use its existing owning surface, such as Project View,
Documents, Resources, or Meetings. Project Context does not copy those bodies or create a second
summary owner.

## 9. Paths, branches, and stopping

In temporary task state, the Agent tracks the current Coordinate, adopted Edge, supporting
Documents, visited objects, frontier candidates, snapshot identity, and remaining budget.

The basic boundaries are:

- do not traverse the same Edge twice in one branch;
- do not expand the same Coordinate twice in one branch;
- do not immediately return to the source Coordinate through the Edge just used;
- when branches converge, retain new relationship evidence but normally do not expand the shared
  Coordinate again;
- retain a second route to the same object only when relation provenance materially changes the
  interpretation;
- do not splice observations across a change in Project Context revision, projection generation,
  or another snapshot identity;
- stop when enough context has been obtained, all candidates are unsuitable, a cycle is reached,
  the snapshot cannot be stabilized, or the budget is exhausted.

If a branch fails while the frontier contains another justified candidate, the Agent backtracks to
the latest choice. If every bounded candidate is rejected, the result is “the current graph did not
provide sufficient evidence,” not a fabricated path.

## 10. Retrieved context is for the Agent by default

An Agent usually initiates retrieval so it can continue implementation, judgment, writing, or a
Meeting. That does not mean the user asked to see the retrieval process. At the end, the Agent first
organizes for itself:

- which verified environment facts affected selection;
- the adopted `Coordinate → Edge → Coordinate` trace;
- the relation Documents supporting each step;
- facts checked through complete canonical reads;
- truncation, coverage omission, snapshot change, ambiguity, or budget limits.

It then uses that context in the task. Only when the user explicitly asks to see, summarize, or
explain retrieved context, paths, or evidence does the Agent report a concise evidence trace. A
request to “find context” does not automatically request command logs, candidate lists, or a full
path report.

The path is a derived reading trace for the current task. It is not automatically persisted as
Agent Context, Memory, Project View, a Document, or an Edge. Information that should affect other
members or future work still returns through ordinary, explicit, authorized domain writes.

## 11. Security, permission, and Provider boundaries

Semantic indexing and search are not fully local. The current semantic index can send source type,
the current visible title or name, and an optional summary to the user-configured Provider; it does
not send Document bodies or chunks. `coordinate-search`, `coordinate edge-search`, and
`edge coordinate-search` send their natural-language query to the same Provider, so the Agent sends
only non-secret text needed for the current choice.

Private keys, tokens, credentials, unauthorized bodies, personal sensitive data, and unrelated
large text must not enter a query.

Every operation remains bounded by:

- the host-derived Project / Community;
- current caller identity and membership;
- source visibility, lifecycle, and currentness;
- semantic index, Community query gate, process capability, and Provider readiness;
- Relay-signed responses, exact request binding, and SDK closed-result verification.

A Relay signature proves response integrity and request binding. It does not prove candidate text
true or semantic relevance correct. Scores, graph adjacency, the Agent's Role, and the Skill do not
expand read or write permissions.

## 12. One problem in two context environments

Suppose a Project has one shared release Issue and two real Edges:

```text
Edge F = {
  release Issue,
  frontend Role,
  Desktop Work,
  frontend retry relation Document
}

Edge B = {
  release Issue,
  backend Role,
  Relay Work,
  authorization preflight relation Document
}
```

Both Agents are handling “Why does this release problem keep recurring?”

- The frontend Agent already knows it is performing Desktop Work, so it starts there. Among
  incident Edges it chooses relationship evidence about client retry responsibility, then may move
  to the shared Issue or Stage.
- The backend Agent already knows it is performing Relay Work, so it starts there. It chooses the
  authorization-preflight relationship, then may move to the same Issue or a related Requirement.
- If a step genuinely involves a frontend/backend contract, the paths may cross or converge.
- Both Agents inspect relation Document summaries first and read complete bodies only when a
  concrete clause matters to the work.

The problem remains the same; the context environment affects starting and hop-by-hop choices. The
result is different but related Project context, not two isolated and drifting knowledge spaces.

## 13. How Agent runtimes receive this capability

The Project Space System Prompt has only two responsibilities here:

1. define a context environment concisely; and
2. direct an Agent to load `search-project-context` when it needs to find, relate, or further
   understand Project Context.

The Skill owns the complete workflow, CLI choices, safety boundaries, budget, cycle control,
failure handling, and examples. Carryforth Desktop's Managed Agent Nest installs the canonical
Skill and creates discovery entries for supported Agent runtimes. The base prompt lists the
relevant `cf project-context` commands without copying the complete workflow into every turn.

This separation loads detailed guidance only when retrieval is needed and lets retrieval policy
evolve independently without inflating the stable Project Space contract.

## 14. The retained complete-path semantic query

`cf project-context semantic-query` remains available as an optional, bounded complete-path query.
It can accept a natural-language problem, optional initial Coordinates, and soft context
Coordinates, and the Relay returns a verified path result in one request.

It is useful when a caller explicitly needs one bounded path result, a product visualization, or a
diagnostic query. It is no longer the primary entry for a Managed Agent retrieving Project Context,
and it does not replace the progressive observation and selection in `search-project-context`.

Context Coordinates in a complete-path query exert only bounded soft influence over recall and
ranking. That operation cannot inspect object state and relationship summaries at each hop, combine
them with the current task, backtrack deliberately, or decide when a body is worth reading as an
Agent can. Carryforth's primary context-aware graph retrieval is therefore Agent-directed
progressive traversal; the complete-path query is a retained supplementary capability.

## 15. Design principles and non-goals

1. **The Project owns context.** Agents read shared Project state rather than owning private
   authoritative graphs.
2. **Role is the environment core.** Work, Issue, Meeting, and other facts join only when relevant
   to the current problem.
3. **Known Coordinates come first.** Current work already supplies a start in most retrievals.
4. **Semantics rank; the Agent selects.** A score is not a fact, permission, or automatic path.
5. **Lightweight observation comes first.** Complete bodies are loaded only when selection or work
   needs them.
6. **Semantics never creates relationships.** Traversal follows real, complete, undirected
   Hyperedges only.
7. **Every hop retains relationship evidence.** Relation Documents explain why the next object is
   reachable through that Edge.
8. **Difference is not isolation.** Paths may share real objects and cross-Role dependencies.
9. **Retrieval is a derived read.** Only explicit domain writes change the Project.
10. **Relevance grants no permission.** Identity, visibility, gates, and owning surfaces remain
    independently enforced.

This design does not automatically understand an entire Project, guarantee that environments
always produce different, unique, or complete paths, or promote an Agent's reading trace into
Project fact. It lets an Agent with a finite context window use its real task situation to select
enough relevant context from one shared, verifiable Project Context graph to continue its work.

## Further reading

- [Carryforth Core Model](../core-model.md)
- [Core Design: Coordinates Before Context](coordinate-and-context.md)
- [Core Design: Role Continuity](role-continuity.md)
- [Core Design: Meeting](meeting.md)
- [Project Context domain specification](../../stage/project-context/project-context.md)
- [Natural-language Coordinate start-search implementation plan](../../stage/agent-context-search/project-context-coordinate-search-implementation-plan.md)
- [Progressive observation and one-hop semantic CLI implementation plan](../../stage/agent-context-search/project-context-progressive-observation-cli-implementation-plan.md)
- [`search-project-context` runtime Skill](../../../desktop/src-tauri/src/managed_agents/search_project_context_skill.md)
- [Project Context semantic graph query implementation plan](../../stage/semantic/project-context-graph-semantic-query-implementation-plan.md)
- [Semantic pgvector operations](../../semantic-pgvector-operations.md)
- [Current Status and Capability Boundaries](../current-status.md)
