# Carryforth Core Model

> This document introduces Carryforth's current product model and the relationships among its
> parts. It does not replace wire schemas, database structures, or authorization protocols; the
> exact contracts remain in the corresponding domain code and design documents.

## 1. Core judgment

Carryforth begins with one principle:

> Continuity belongs to the project, not to any single agent.

The project persists. Humans, agents, Leaders, and concrete runtimes may enter, leave, stop, or be
replaced. What must persist is the project's own description, goals, responsibilities, work,
documents, context, important choices and their rationale, and current situation.

In the current implementation, one Carryforth Community forms the identity, authorization, and
data boundary of one Project. A Community is not the whole product meaning of a Project, but it
provides the root project identity, member admission, tenant scope, and base permissions.

```text
Project / Community
│
├── Project View            First-order current project state
├── Role Continuity         Stable responsibility and replaceable executors
├── Project Documents       Revisable project content
├── Project Context         Second-order semantics across objects
├── Meetings                Formal collaboration and outcomes
└── Members                 Humans and agents
```

## 2. Project View

Project View is the direct, visible surface of a project at the current point in time. It lets a
member answer without complex inference: What is this project? What is it trying to do? Where is
it now? Which stable responsibility positions exist? Which requirements, issues, work items, and
resources are current?

Project View contains nine kinds of stably referenceable objects.

### 2.1 Project Profile

Project Profile is the Project's unique one-to-one descriptive surface. It answers:

- What is this project?
- Why does it exist?
- What problem is it trying to solve?
- What is its basic scope?

The Project is the long-lived root object. Project Profile is the viewable and revisable
description of that Project. They are not the same object, and changing Profile content does not
change Project identity.

### 2.2 Goal

A Goal expresses a high-level outcome the project intends to reach. A Project has at least one
Goal and may maintain multiple parallel Goals.

A Goal may organize zero or more Plans. The system does not automatically determine whether a
Goal has been achieved, nor derive Requirements, Issues, or Work from it.

### 2.3 Role

A Role is a stable, identifiable semantic responsibility position within a Project. It states why
the position exists, what it is responsible for, and what lies outside its boundary.

A Role is not a Persona, model, process, session, or member. “Who currently bears this Role” is
expressed by an Assignment in Role Continuity; the Role itself remains stable.

### 2.4 Plan

A Plan expresses the logic and structure by which a project intends to advance. It may be linked
to a Goal, or exist independently as a Project Plan without a Goal.

A Plan organizes Requirements and Issues through Stages. Linking a Plan to a Goal describes its
current organizational position; it does not imply that completing the Plan is automatically
sufficient to achieve the Goal.

### 2.5 Stage

A Stage is a stable coordinate within a Plan for expressing segmentation, position, or progress.
Every Stage belongs to a Plan, but Stages need not form a single linear sequence; they may express
parallel or branching structure.

Requirements and Issues can be placed into a Stage. The Stage itself does not automatically
change status when those objects change.

### 2.6 Requirement

A Requirement expresses what the project wants to implement, change, or satisfy. It may remain
unplanned, or be placed into a Stage and handled by zero or more Work items.

A Requirement answers “what should be achieved,” unlike an Issue, which answers “what problem has
already been observed.”

### 2.7 Issue

An Issue expresses a known problem, gap, anomaly, feedback item, or blocker.

An Issue may:

- remain unplanned or be scheduled into a Stage;
- point through `about` to a Project View object, identifying where the problem occurs;
- be handled by zero or more Work items.

Planning position, problem location, and actual handling Work are three independent dimensions.
The system does not derive an owner, Stage, or solution merely from an `about` relationship.

### 2.8 Work

Work is the fundamental execution unit arranged by the Project to handle a Requirement or Issue.
Humans and agents accept and perform Work; they do not directly “execute a Goal” or “bear a Stage.”

Every Work item has exactly one primary subject: one Requirement or one Issue. If an action appears
to handle multiple subjects, split it into separate Work items or identify one unambiguous primary
subject.

### 2.9 Resource

A Resource expresses a stable project-related entry point, such as a code repository, design,
document, service, environment, or existing artifact.

In v3, a Resource does not store a locator itself. It consists of a `name`, an open
`resource_kind`, an optional summary, and a required `guide_document_id`. The Guide is an ordinary
Project Document that explains how to locate and use the Resource.

A Resource is not a copy of the external asset. Registering it does not install tools, reveal
secrets, or execute code. An existing NIP-34 Repository is a Repository Resource in Project View;
it is not the Project itself.

### 2.10 Project View Context Reference

An active Project View object other than Resource may carry normalized Context References to
related Resources or to a live or pinned Revision of a Project Document. A Resource may reference
only a Document, not another Resource. A Context Reference only says “this asset is relevant to
this object.” It does not authorize access, install or execute anything, or copy referenced
content into object fields.

A Project View Context Reference and the Project Context Edge in section 6 are different
relationships:

- a Context Reference is a lightweight related-asset reference directly owned by one Project View
  object;
- a Context Edge is an undirected Hyperedge shared by two or more Project coordinates and
  explained by Context Documents.

Neither is automatically derived from the other. A Resource's primary Guide is also expressed by
the separate `guide_document_id`; it cannot be inferred from ordinary Context References.

### 2.11 Primary Project View relationships

The following graph shows a common reading order, not UI nesting or database ownership:

```text
Goal?
  └── Plan
       └── Stage
            ├── Requirement
            │    └── Work[]
            └── Issue
                 └── Work[]

Issue ── about? ──> any Project View object
```

- A Plan may have no Goal.
- A Requirement or Issue may have no Stage.
- A Stage always belongs to a Plan.
- Work always handles one Requirement or Issue.
- An Issue's `about` reference neither copies the target nor produces implicit business-state
  changes.

See the [Project View object relationship design](../stage/project-view/object-relation-design.md)
for complete relationships and cardinalities.

## 3. Role Continuity

Role Continuity answers how responsibility and work continue when the assignee or runtime changes.

```text
Role                   Stable responsibility position
  └── Assignment       Who currently bears it
       └── Member      Who makes the commitment
            └── Runtime  Who is currently executing

The Project continues to preserve Work, Checkpoints, Handoffs, and canonical state
```

In this model:

- Role expresses a responsibility position;
- Assignment identifies which Community Member currently bears it;
- Member is a Human or Agent with a stable public-key identity;
- Runtime is a short-lived execution instance of that Member;
- Checkpoints and Handoffs leave current situation, risk, and unfinished responsibility in the
  Project;
- Role Brief is a derived read of canonical project state, not a second source of truth.

Restarting a runtime under the same Agent public key preserves the Member. Changing to another
Agent public key creates another Member and requires a new Assignment and explicit transition.
Changing Persona, model, or Provider does not automatically change member identity.

The current continuity contract also preserves these boundaries:

- one Role has at most one active Assignment;
- one Member has at most one active Assignment;
- one Work item has at most one responsible Role and one active Commitment;
- ending an Assignment does not automatically complete, cancel, or reassign Work;
- a successor explicitly continues responsibility through a new Assignment and Commitment;
  Handoff is a useful entry point, not a prerequisite for continuity.

An active Project Member can therefore be understood as a Community Member who holds an active
Role Assignment. A candidate agent, a connected runtime, or Channel membership is not sufficient
to create an active Assignment.

See [Core design: Role Continuity](core-design/role-continuity.md) for why responsibility, tenure,
Work Responsibility, Commitment, and Runtime are separated, and how continuity works without a
predecessor's exit summary. The exact domain contract remains in
[Role Continuity](../stage/role/role-continuity.md).

## 4. Members: Humans and agents

Humans and agents enter a Project as Community Members and use the same project objects and
collaboration model. An agent is not a hidden temporary function beneath a Leader; it may have a
stable identity, Role, Assignment, and attributed history.

This does not mean every member has equal power. The Community's base `owner`, `admin`, and
`member` levels, along with domain capability gates, signatures, and state checks, still determine
what a member may observe, propose, write, or approve.

Humans retain governance responsibility for project goals, value boundaries, permissions,
high-risk matters, and irreversible decisions. Treating an agent as a first-class member does not
remove ultimate human responsibility.

## 5. Project Documents

A Project Document is a Markdown document with a stable `document_id`. Every save produces an
immutable full Revision and advances an explicit current Revision.

It is suitable for:

- designs, constraints, and operating instructions;
- decision rationale and applicability boundaries;
- Meeting outcomes;
- explanatory Project Context content;
- project understanding that future members must inherit.

Document identity does not depend on title or one event ID. Concurrent conflicts are neither
silently overwritten nor automatically rebased. Deletion is represented by a verifiable
tombstone.

A Project Document is a durable project record, not secret storage. API keys, private keys, real
credentials, and user content that should not be shared do not belong in it.

See [Project Document](../stage/document/document.md) for the full design.

## 6. Project Context

Project View answers first-order questions: What is this? What exists? Where are we now? Project
Context preserves second-order semantics: Why are these things related? Which special dependencies
exist? What might be affected? Where does the explanation apply?

The minimal model is one undirected Edge or Hyperedge:

```text
ProjectContextEdge
├── coordinates            two or more project coordinates
└── context_documents      one or more explanatory Documents
```

Current coordinates may reference:

- a Project View object;
- a Project Document;
- a Meeting.

An Edge expresses only the structural fact that “this exact set of coordinates shares context.”
Ordinary Project Documents carry the actual explanation. One exact coordinate set has one Edge
within a Project, while that Edge may bind multiple explanatory Documents.

Edges and Context Documents also preserve these lifecycle constraints:

- one Project Document may serve as a Context Document for at most one Edge;
- an active Edge has at least one Context Document; detaching the last one removes the active Edge;
- tombstoning a coordinate object does not silently shrink or delete an existing Edge—the
  historical relationship retains its original coordinate identity.

The system does not infer or create these relationships automatically from text. Humans and agents
explicitly maintain Edges and Context Documents as they discover real project semantics. The Relay
validates coordinates, project boundaries, and reference integrity.

Desktop currently provides a read-only relationship canvas, inspector, and live updates. Canonical
Edge attach/detach operations primarily use `cf` and agent operations.

See [Core design: Coordinates before context](core-design/coordinate-and-context.md) for why Context
starts from stable object identities and why Coordinate, Edge, and Document have separate
responsibilities. The full domain semantics remain in
[Project Context](../stage/project-context/project-context.md).

### 6.1 Semantic graph-path queries

The optional semantic graph query operates on a verified Project Context graph. A caller may
provide:

- a natural-language problem;
- optional initial coordinates;
- optional Role, Work, or other query-context coordinates that influence recall and ranking.

The Relay signs the result Event and binds it to the current Project, caller, and exact request
body. The result also carries source, graph-snapshot currentness, and Revision evidence. After
verification, `cf` separately derives unsigned but normalized `read_commands` that let the caller
read canonical objects. The result DTO does not copy source-document bodies into the response.

This capability requires separate semantic Provider configuration, an index generation, durable
Community index/query gates, and acknowledgement that the problem leaves the local system. Its
relevance, resource isolation, long-term stability, and production deployment remain under
qualification. Supplying Role or Work context only means that it participates in recall and
ranking; it does not guarantee one uniquely human-expected answer for every problem.

See [Core design: Context-aware semantic graph retrieval](core-design/context-aware-semantic-graph-retrieval.md)
for why this capability uses one Project Context graph, how a context environment differs from a
context path, and why Carryforth does not create private agent context. See
[Semantic pgvector operations](../semantic-pgvector-operations.md) for activation and operations
boundaries.

## 7. Meetings

A Meeting is a bounded formal collaboration object, not a collection of loose chat messages.

The current V2 model includes a fixed roster, moderator, agenda, shared Board, Floor, speech
timeline, Handoff, moderator decisions, leases and timeouts, close or abort, and Action
Finalization. Humans and agents may participate together, and the system gives Human Floor
requests priority.

Important Meeting outcomes should return to the Project as Work, Documents, Context, Checkpoints,
or other canonical state instead of remaining only in the Meeting runtime.

Board, Speech, Close, and moderator text do not automatically modify Project View or establish a
project decision. A Meeting outcome becomes canonical state only when an authorized member
explicitly writes and reads it back through an existing business surface or ordinary domain
command. Action Finalization is a bounded stage in which the moderator performs those ordinary
operations and submits an `actions-recorded` ACK; it is not a special materializer that writes
business state on behalf of the Meeting.

Meeting remains a preview capability. Creation, direct action, and Community read each have
independent switches and authorization. Default visibility and later read expansion cannot be
changed by client declaration alone.

See [Core design: Meeting](core-design/meeting.md) for why Meeting uses distributed context, a
shared Board, and explicit action closure. Exact phase semantics remain in
[Meeting V2](../stage/meeting/v2/meeting-v2.md).

## 8. A typical collaboration

1. A Human starts Carryforth locally and creates or enters a Project / Community.
2. Humans and agents join with stable identities and read the goals, roles, plans, and current Work
   in Project View.
3. A member responds to a Role Proposal. Once both candidate acceptance and governance
   authorization are satisfied, an active Assignment is formed. Runtimes can be replaced without
   losing responsibility or project records.
4. Members read Documents, Resource Guides, and relevant Project Context on demand instead of
   placing all material into every conversation.
5. Everyday discussion happens in Channels. Conclusions that affect the project's future are
   explicitly written back to objects, Documents, Context, or Checkpoints.
6. When formal discussion is needed, members start a Meeting whose moderator maintains the agenda,
   shared state, and outcome.
7. When an agent leaves or is replaced, a new runtime continues through the Role Brief, current
   Work, Documents, Context, and history.

The point is not to preserve more text. It is to let the project own durable state that can be
verified, read, revised, and handed over.

## 9. Implementation and activation boundaries

These models have corresponding protocol, Relay, CLI, or Desktop implementations, but they are not
all enabled automatically in a new environment:

- Projects, Project View (including Documents and Context), and Meetings are preview features in
  Desktop;
- Project View v3 requires Relay-operator preparation followed by Community-owner review and
  signed initialization;
- Documents, Context, Meetings, and semantic queries each have their own readiness and durable
  gates;
- `./start.sh` starts the local source stack but does not replace those governance and
  authorization actions;
- code existing in the repository does not mean production qualification is complete.

See [Current status](current-status.md) for current maturity and support boundaries.
