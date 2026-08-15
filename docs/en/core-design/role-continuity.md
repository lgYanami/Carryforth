# Role Continuity: Keeping Responsibility Alive Across Agent Tenures

> This document explains one of Carryforth's core designs: how Project-held Roles, Assignments,
> Work Responsibility, Commitments, Checkpoints, Handoffs, and Role Briefs allow responsibility
> and work to continue when Humans, Agents, models, Sessions, or Runtimes change, while preserving
> truthful attribution for every tenure and contribution.
>
> This document describes the product mental model. It does not redefine events, database
> constraints, CLI behavior, or the Runtime supervisor protocol. See the
> [Role Continuity domain contract](../../stage/role/role-continuity.md) for the precise domain
> contract.

## 1. Core principle

> Role Continuity does not transfer a predecessor Agent's memory to a successor. It lets the
> Project continuously preserve responsibilities, assignment tenures, work commitments, and the
> situation that has been externalized, so a successor can reconstruct the responsibility context
> from Project state.

In a long-running project, the assignees and execution vehicles change:

- Human or Agent Members join, leave, or are replaced;
- the model, Provider, or Persona used by an Agent changes;
- Sessions end and context windows are compacted;
- Runtimes stop, fail, recover, or are rebuilt;
- a Leader is not a permanent single point either.

But the responsibilities a Project needs do not disappear automatically when those things change.
A module still needs maintenance, a Work item still needs progress, and unresolved blockers, risks,
and next steps still belong to the Project.

```text
Project
│
├── Role                         Long-lived, stable responsibility position
│
├── Assignment A                One tenure in which Member A holds the Role
│   ├── Work Commitment         Work explicitly accepted by A during this tenure
│   ├── Checkpoints             The situation A continuously externalized
│   └── Handoff                 Optional transition supplement
│
├── Assignment B                Member B's new tenure
│   └── New Work Commitment     B explicitly continues the unfinished Work
│
└── Role Brief                  Continuation view derived from current canonical state
```

This mechanism does not try to extend the life of a process. It moves continuity out of processes,
sessions, and individual memory and back into the Project.

## 2. Separate the five questions first

Role Continuity works because it does not use one "current assignee" field to represent everything at
once.

| Model | Question it answers | State owner |
|---|---|---|
| Role | What long-lived responsibility position does the Project need? | Project |
| Assignment | Which Member currently holds it, and in which tenure? | Project |
| Work Responsibility | Which Role remains responsible for this Work over time? | Project / Work |
| Work Commitment | Which Assignment's Member explicitly accepted it during this tenure? | Project |
| Runtime | Which short-lived execution instance is currently running? | Runtime control plane |

Checkpoint, Handoff, and Role Brief address three further concerns:

- **Checkpoint**: continuously externalizes the current situation during a tenure;
- **Handoff**: adds an entry point and unresolved items when a transition is needed;
- **Role Brief**: regenerates a bounded continuation view from current canonical state.

These concepts are not interchangeable. A Role is not an Agent, an Assignment is not a Runtime, a
Commitment is not Work state, a Checkpoint is not private Role memory, and a Role Brief is not a
second source of truth.

## 3. Role: a stable responsibility position held by the Project

A Role expresses a long-lived, stable, and identifiable responsibility boundary within a Project:

- why it exists;
- what it is responsible for;
- what it is explicitly not responsible for;
- whether it is currently active;
- its current governance level.

A Role does not directly store:

- its current assignee;
- an Agent Runtime, Session, model, or Persona;
- the execution state of Work;
- a Member's free-form memory.

For example, a "Desktop Lead" can remain in the Project over time. Agent A may hold it today and
Human B or Agent C in the next stage. The Role identity, responsibilities, and responsible Work do
not need to be renamed when the assignee changes.

```text
Role: Desktop Lead
  purpose          Keep the Desktop experience consistent with Relay contracts
  responsibilities Interaction, recovery paths, and client verification
  boundaries       Must not unilaterally change Relay authorization contracts
```

A Role should correspond to a stable responsibility boundary, not be created for every temporary
task. Fixing one timeout is Work; long-term maintenance of the Desktop recovery experience is an
appropriate Role.

At present, a Role can have at most one active Assignment at a time, and a Member can also have at
most one active Assignment at a time. Over time, however, both can have multiple immutable tenures.

## 4. Assignment: a bounded tenure in which a Member holds a Role

An Assignment means:

> A Community Member with a stable public-key identity formally holds a Role for a defined tenure.

"Bounded tenure" does not mean that an expiration time must be set in advance. It means that each
tenure has its own independent, non-reusable Assignment identity and explicit start and end facts.

```text
Role R
  ├── Assignment A1: Member A, 2026-07 → 2026-08
  └── Assignment A2: Member B, 2026-08 → active
```

An Assignment is bound to the Member's own stable public key, not to:

- a Human owner;
- a Persona;
- a Team;
- a model or Provider;
- a process ID;
- a particular Session;
- a particular Runtime.

Therefore:

- changing the model, Provider, Session, or Runtime for the same Agent public key does not
  automatically change the Member or Assignment;
- changing to another public key means another Member and requires a new Assignment;
- stopping, disconnecting, or restarting a Runtime does not automatically end an Assignment;
- an ended Assignment is never reactivated or reused by a successor.

### 4.1 Proposal precedes Assignment

Applications, invitations, and negotiations first create a Role Assignment Proposal. A Proposal
expresses only candidate intent and Project authorization state; it does not itself grant Role
authority.

A Request already expresses candidate acceptance when it is created. An Offer already expresses
governor authorization when it is created, while the other side must still provide its own
confirmation. The current system only forbids multiple open Proposals for the same
`(Role, candidate)` pair: one Role may have multiple candidates, and one candidate may have
Proposals for multiple Roles. The strict one-to-one constraints apply only to active Assignments.
After one Proposal completes, other Proposals may remain open, but their stale consistency fences
cannot bypass current Assignment state to activate.

The Project creates an active Assignment atomically only when candidate acceptance, current
governance authorization, and the Project consistency fence all hold. A Member's self-declared
Role, a client tag, joining a Channel, or starting a Runtime cannot replace this process.

### 4.2 Assignment is the authorization coordinate for Role-bearing actions

When an operation is performed on behalf of a Role—for example, appending a Checkpoint or Handoff,
accepting a Work Commitment, or exercising Leader governance—the system requires the signer to
hold the corresponding exact active Assignment.

Assignment is not a universal ACL for all ordinary Project content reads and writes. Ordinary
Project View, Document, and Context operations remain independently authorized by Community
membership, operation-specific authority, object state, and domain gates.

### 4.3 Current governance root and Leader boundaries

The Community owner is the sole governance root. That authority is not granted by a Role and does
not require an Assignment. An Active Leader, by contrast, must have both Community `admin` status
and an exact active `level=admin` Assignment.

The current governance boundaries are:

- the owner may govern `admin` and `member` Roles / Assignments;
- an Active Leader may govern only ordinary `member` Roles / Assignments;
- only the owner may create an `admin` Role or change its level or lifecycle;
- a Leader cannot end its own Assignment or that of a peer Leader;
- a verified human owner may end the Assignment of a managed Agent it owns, including a Leader
  Assignment;
- a managed Agent Leader cannot use textual Role responsibilities to govern Humans or unknown
  principals;
- all current Leaders map to Community-level admin, with no domain ACL derived from textual Role
  descriptions.

Consequently, the responsibility description of a "Frontend Leader" or "Backend Leader" does not
automatically constrain its existing admin authority. Domain-level authority would require a
separate future design and cannot be inferred from Role text.

### 4.4 An assignee cannot rewrite governance facts by "stopping work"

The current implementation forbids every assignee, Human or Agent, from unilaterally ending its
own Assignment. An assignee may request replacement or report that it cannot continue, but the
Assignment remains active until an authorized governor completes a replacement or revocation, or
a trusted recovery process determines that it is unrecoverable.

This prevents an Agent from dropping Project responsibility into a void merely by shutting down a
Runtime, disconnecting, or submitting a self-reported status.

## 5. Work Responsibility and Commitment: responsibility and commitment must be separate

This is one of the most important separations in Role Continuity.

```text
Work
├── responsible Role             Project responsibility that persists across Assignments
└── active Work Commitment       Explicit acceptance by one current Assignment
```

### 5.1 Work Responsibility persists across tenures

`responsible_role_id` answers: **Which stable Role is responsible for this Work?**

It belongs to the Work's current Project state, not to the current assignee. One Work item has at
most one responsible Role, while one Role may be responsible for multiple Work items.

Setting or clearing the responsible Role is a governance action that only the Community owner or
an eligible Active Leader may perform. An ordinary Member cannot bypass this responsibility
boundary by editing Work. While a Work item has an active Commitment, the system forbids directly
setting, clearing, or reassigning its responsible Role. The exact assignee must first release the
Commitment, or a valid Assignment lifecycle transition must first end it. Moving Work to a terminal
state also closes its Commitment, but terminal Work cannot be committed to again.

### 5.2 Commitment belongs only to a specific tenure

A Work Commitment answers: **Which Assignment's Member explicitly accepted this Work during this
tenure?**

A Work item can have at most one active Commitment at present, while an Assignment may hold
multiple active Commitments. A Commitment requires all of the following:

- the Work remains executable;
- the Work has a responsible Role;
- the Assignment remains active;
- the Assignment's Role matches the responsible Role;
- the signer is the Member of that Assignment.

A Commitment is not:

- ownership of the Work;
- proof that the Work is complete;
- a Runtime execution lock;
- a promise that can be transferred between Members;
- a substitute for Work state.

Commitment end reasons are also precise: an assignee explicitly ending one produces `released`; an
atomic recommitment by the same Assignment produces `replaced`; ending the Assignment produces
`assignment_ended`; and moving Work to a terminal state produces `work_closed`.

### 5.3 A successor continues responsibility without inheriting the predecessor's commitment

When an Assignment ends:

- the Work does not automatically become completed, cancelled, or reassigned;
- the responsible Role remains unchanged;
- the predecessor's Commitment ends as `assignment_ended`;
- the predecessor's Assignment, Commitment, and historical contributions remain attributed to the
  predecessor;
- the successor must create a new Commitment through its own new Assignment.

```text
Work W ──responsible──> Role R

Assignment A / Member A
  └── Commitment C1 ──ended(assignment_ended)

Assignment B / Member B
  └── Commitment C2 ──active
```

The Project can therefore continue to say, truthfully, that "Role R remains responsible for Work
W" while also saying that "Member A made Commitment C1, and Member B now continues the Work
through C2."

## 6. Checkpoint: replace exit summaries with continuous externalization

If all progress exists only inside an Agent's internal context, Role Continuity will still fail
when the Runtime disappears.

A Checkpoint uses structured, append-only records to continuously externalize the current
situation of a Role, including:

- a concise situation summary;
- current areas of focus;
- progress and evidence;
- blockers;
- risks;
- unresolved questions;
- next steps;
- typed references to Work, Issues, Assignments, Commitments, or Project events.

A Checkpoint should be created when important changes occur during the work, not only as an exit
summary when a Member leaves.

### 6.1 A Checkpoint is append-only history, not mutable Role Memory

Each append creates a new `checkpoint_id` and Project revision. Old Checkpoints are never edited or
deleted.

`supersedes_checkpoint_id` only means that the same author, through the same Assignment for the
same Role, appended a correcting record. The corrected entry remains in canonical history. A Role
Brief selects the latest Checkpoint as the current entry point, while the complete history remains
available through paginated reads.

### 6.2 A Checkpoint baseline means "how far the author reviewed"

`based_on_project_revision` records the Project revision the author reviewed when creating the
Checkpoint. It is not a guarantee that the Checkpoint remains current forever after that. The
Project may continue to change after the Checkpoint lands, so readers must compare the
Checkpoint's own `project_revision` with the current Project head to assess freshness.

### 6.3 A Checkpoint does not duplicate canonical facts

A Checkpoint organizes the situation; it does not replace the owners of those facts:

- Work status is still updated on Work;
- blocking problems are still updated on Issues;
- long-form content, designs, and evidence still belong in Documents;
- cross-object causes and effects still belong in Project Context;
- external execution results remain in the external system of record.

For example, a Checkpoint may say, "the database resource gate still blocks acceptance," and
reference Issue I-7 and remediation Document D-4. It cannot declare I-7 closed only inside the
Checkpoint without updating the Issue.

## 7. Handoff: improve transition quality without making it a prerequisite

A Handoff is an append-only transition record that may reference:

- the source Assignment;
- the target Assignment, if a direct replacement has already occurred;
- the latest Checkpoint;
- affected Commitments;
- unresolved items and related Project references;
- the transition cause.

There are currently two kinds of Handoff:

### 7.1 A planned transition supplement authored by a Member

During its tenure, an active assignee may append a `planned` / `other` Handoff with richer context
and unresolved items. This record **does not end the Assignment** and does not transfer Work or
authority to another Member.

### 7.2 A minimal cutover record generated by the Project

When a formal replacement or trusted `unrecoverable` process ends an Assignment, the current
implementation generates a minimal system Handoff. It references the source Assignment's latest
Checkpoint, the Commitments that ended, and Work awaiting continuation. An ordinary revocation
does not automatically generate a Handoff. `membership_ended` and `role_deactivated` are reserved
end reasons in the current model, but their presence does not imply that all corresponding paths
currently generate Handoffs automatically.

A complete Handoff submitted by the predecessor is therefore not required for replacement. The
old Member may be unreachable, refuse to summarize, or no longer be able to run. The Project must
still be able to recover from Work, Issues, Documents, Context, Checkpoints, and system cutover
records that were continuously written back.

> Handoff improves a transition, but continuity depends on the Project already holding enough
> canonical state during normal work.

## 8. Role Brief: recompile a continuation view from Project state

A Role Brief is a bounded, derived read for a current assignee or candidate. It is not persistent
Role Memory.

The current v3 machine-readable Brief can combine:

- the Project, projection generation, Project revision, and membership snapshot;
- the Project Profile and Goals;
- a bounded active Role directory;
- the current Assignment or open Proposals;
- non-terminal responsible Work and its committed / waiting state;
- Role-related Issues and Work that handles them;
- the latest Checkpoint for the Role;
- the three most recent Handoffs;
- bounded one-hop Context and Document metadata / fetch commands;
- signed projections and currentness boundaries for each source.

```text
Role Brief =
  verified Project snapshot
  + Role / Assignment
  + responsible Work / Commitment view
  + latest Checkpoint / recent Handoffs
  + bounded related objects and Context
  + source revisions
```

The Role Brief recompiles state distributed across the Project into a minimal, verifiable Role
perspective. It can be rebuilt and must not override canonical Project View, Document, Context, or
Role Continuity facts when they conflict.

### 8.1 A Role Brief is not complete memory

A Role Brief does not contain:

- the predecessor's full conversation history;
- internal reasoning that an Agent did not externalize;
- the bodies of every Project Document;
- the complete Checkpoint / Handoff history;
- automatically inferred facts or authority.

It only provides an entry point for continuing work. When more material is needed, the successor
uses canonical reads, Role history, Documents, and Project Context to expand the relevant material
on demand.

### 8.2 Role Brief currentness has an explicit snapshot boundary

A Brief is assembled from Relay-signed projections and exact Project meta, generation, membership,
Member, and Relay identities. It represents only the snapshot verified when it was generated. If
the Project head changes, the Brief must be resolved again and cannot be reused indefinitely.

The client-side `generated_at` is the assembly time, not Relay canonical write time.

## 9. Three continuation scenarios

### 9.1 The same Member changes Runtime, Session, or model

```text
Member A + Assignment A
       │
Runtime 1 / Model X ends
       │
Runtime 2 / Model Y starts
       │
Re-read the current Role Brief
       │
Continue the same Assignment and existing Commitments
```

The Member public key and Assignment have not changed, so no new tenure is needed. The new Runtime
reads current Project state again, but it cannot automatically inherit thoughts or temporary
context that the old model did not write back.

### 9.2 Planned replacement of the assignee

```text
Old Assignment active
  → New candidate reads the required Role Brief
  → Proposal receives candidate acceptance and governance authorization
  → Atomically end the old Assignment / activate the new Assignment
  → End predecessor Commitments while preserving attribution
  → Successor explicitly accepts unfinished Work
```

A planned Handoff can provide additional context, but cutover is not blocked if the predecessor did
not submit one.

### 9.3 Unplanned interruption

For a managed Runtime that can be supervised reliably, the Project can attempt recovery first. It
may end the Assignment only when objective unrecoverable conditions are met, the corresponding
policy is explicitly enabled, and evidence is retained. Automatic `unrecoverable` is currently
disabled by default.

This recovery state machine and its Runtime evidence do not mean that "the system promises to
restart remote processes automatically in every deployment." Actual process-recovery capability
depends on the deployment and supervisor implementation. Role Continuity only defines when Runtime
evidence may be used for a conservative Assignment recovery decision.

An external Agent that cannot be supervised reliably cannot be assumed to have crashed merely
because it has not sent a recent message. An authorized principal must explicitly handle the
Assignment before a successor recovers the situation from Project state.

## 10. Truthful attribution: continue responsibility without rewriting contributors

Role-bearing state actively created by a Member preserves at least two layers of attribution:

```text
Member public key   Answers "who did this?"
Assignment ID       Answers "through which Role and during which tenure?"
```

A Runtime ID, epoch, or lease may provide additional operational evidence, but cannot replace the
Member and Assignment. A system Handoff is explicitly marked as system-generated: it references
the source Assignment, but its `created_by` is empty and it must not be described as a contribution
personally submitted by the predecessor.

After a Member is replaced:

- the Role identity remains stable;
- the Work identity and responsible Role remain stable;
- the new Member receives a new Assignment;
- new Commitments are attributed to the new tenure;
- Checkpoints, Handoffs, Commitments, and business writes actively submitted by the predecessor
  remain attributed to the predecessor;
- a system Handoff preserves a system-generated fact and source Assignment association without
  pretending to be a predecessor-authored contribution;
- the successor continues the responsibility without pretending to have performed the
  predecessor's work.

The Project therefore gains two forms of continuity at once: responsibility can continue, while
historical attribution remains intact.

## 11. Runtime supervision is not Role authorization

Runtime supervision, bindings, leases, and fences belong to the Runtime control plane. They support
operational evidence, recovery, epoch / lease coordination, maintenance, and optional provenance
attribution.

The current authorization boundaries are:

1. Community admission;
2. operation-specific authority;
3. the exact active Assignment for Role-bearing actions;
4. Runtime attribution validation only when a command explicitly carries a Runtime fence.

Omitting Runtime attribution neither grants nor revokes otherwise-valid business authority. Once
explicitly supplied, it must exactly match the active binding, Runtime ID, epoch, and unexpired
lease.

Registering and revoking a binding belongs to the Relay operator control plane. An Agent or Desktop
cannot grant itself that capability. A missing binding / key, mismatch, expired state, or unknown
supervision state clears the current fence and degrades ordinary Role work to unsupervised
operation. These conditions do not block an already verified Role Brief or ordinary Role-bearing
operations.

The following capabilities still depend strictly on the supervisor: Runtime evidence, epoch /
lease, automatic `unrecoverable`, and maintenance drain, freeze, and ACK. Missing supervision does
not expand those capabilities, and the fail-closed maintenance boundary cannot be bypassed merely
because ordinary Role operations remain available.

### 11.1 The current contract does not guarantee exclusive single-Runtime writes

The current contract does not guarantee that only one process can write through an Assignment at
a time. If an old process still holds the same Member private key, the Assignment remains active,
and the command does not explicitly carry a Runtime fence, it may submit Role-bearing commands in
parallel with a new process. Project revision CAS, receipts, and append-only history provide
conflict detection and auditability, but do not guarantee exactly-once or a single Runtime writer.

After an Assignment ends, Role-bearing commands from the old tenure are rejected. If the Member
still has Community eligibility, it may still perform operations that require only ordinary
Community authority. Ending an Assignment does not revoke all Project access from the Member.

## 12. A complete example

Suppose a Project has a long-lived Role named `Desktop Lead`, responsible for two Work items:

- `W1: Fix semantic-query timeout state`;
- `W2: Complete Context graph interaction acceptance`.

Agent A holds the Role through Assignment A and creates Commitments C1 and C2 for the two Work
items. As the work proceeds, it continuously appends Checkpoints recording what has been fixed,
what remains blocked, where the acceptance evidence lives, and what comes next.

Later, the Project replaces Agent A with Agent B through an atomic replacement:

1. `W1`, `W2`, and their responsible Role remain unchanged;
2. Assignment A ends, and C1 and C2 end as `assignment_ended`;
3. the Project creates a minimal Handoff that references Assignment A's latest Checkpoint and the
   two affected Commitments, explicitly marking it as system-generated rather than submitted by
   Agent A;
4. Agent B takes over through a new Proposal / Assignment B;
5. the Role Brief lists the Role, unfinished Work, latest Checkpoint, recent Handoffs, and related
   Context from current Project state;
6. Agent B creates its own Commitments C3 and C4;
7. A's historical contributions remain attributed to A, while B continues from the current state.

No step transfers Agent A's internal memory or rewrites C1 / C2 as commitments made by Agent B.

## 13. Current implementation boundaries

The current code implements:

- strict active Assignment cardinality for Roles and Members, plus uniqueness of an open Proposal
  for the same Role / candidate pair;
- separation of Work Responsibility and Work Commitment;
- Assignment replacement and non-reusable history;
- append-only Checkpoints / Handoffs;
- minimal system Handoffs for replacement / recovery;
- the v3 Role Brief JSON verified snapshot, latest Checkpoint, recent Handoffs, and bounded Context;
- separation of Role-bearing Assignment authorization from optional Runtime attribution;
- binding / lease revocation after Assignment end, and rejection of old-tenure actions.

The current automatic continuation chain still has two explicit gaps:

- v3 machine-readable Role Brief JSON already contains `latest_checkpoint`, `recent_handoffs`, and
  `related_objects`;
- the current Markdown renderer used by the ACP full prompt and `cf roles brief --markdown` does
  not yet render those fields.

It is therefore inaccurate to claim that "a new Runtime already receives every Checkpoint and
Handoff in its Prompt automatically." This state exists and can be read through JSON or explicit
Role history, but the default Markdown / ACP injection still needs to be completed.

The current full Role Brief Markdown also retains old wording such as "Assignment plus current
Runtime fence is the write fence," while the compact Role Binding tells callers to resolve a
Runtime fence before every write. This conflicts with the current contract in which Assignment
grants authority and Runtime attribution is checked only when explicitly supplied. ACP appends a
separate, correct explanation that supervision is not business authorization, but the same Prompt
can still contain contradictory guidance. Until the renderer is fixed, callers must follow the
current Relay / DB authorization contract and must not interpret the old guidance as requiring a
mandatory Runtime fence.

There is another read boundary: Relay atomically accepts Checkpoint / Handoff appends and produces
signed projections, but the general `cf roles checkpoint/handoff append` commands do not currently
enforce a post-write projection readback. The accurate statement is "the write can be read back
canonically," not "the CLI has automatically completed readback proof."

## 14. Non-goals

Role Continuity does not attempt to:

- preserve or transfer complete Agent Sessions, hidden reasoning, or private drafts;
- give a Role free-form memory that duplicates Project state;
- merge Role, Member, Assignment, Persona, model, and Runtime into one identity;
- let multiple active Members hold one Role at the same time;
- let one Member hold multiple active Roles at the same time;
- let a Runtime stop, silence, or lease expiration automatically end an Assignment;
- let an assignee unilaterally relinquish an Assignment;
- treat a Commitment as Work completion, ownership, or an execution lock;
- automatically transfer a predecessor's Commitment to a successor;
- require a predecessor's exit summary before replacement;
- guarantee only one model process per Assignment;
- guarantee exactly-once external execution;
- let Role Brief replace canonical objects or complete Project history.

## 15. Design principles derived from this model

1. **Responsibility belongs to the Project.** A Role does not disappear with an assignee, Session,
   or Runtime.
2. **Separate responsibility positions from assignment tenures.** A Role is stable; an Assignment
   is bounded and non-reusable.
3. **Separate long-lived responsibility from specific commitment.** The responsible Role persists
   across tenures; a Commitment belongs to a specific Assignment.
4. **Continuation does not rewrite history.** A successor creates a new Assignment / Commitment,
   while the predecessor's contributions retain their original attribution.
5. **Continuous externalization takes priority over exit summaries.** Append a Checkpoint when
   important changes occur, not only when leaving.
6. **Handoff is an enhancement, not a dependency.** The Project must remain recoverable without a
   predecessor summary.
7. **A Brief is a derived entry point, not a second source of truth.** Every important item can be
   traced back to signed projections and canonical objects.
8. **Runtime is an execution vehicle, not a source of business authority.** The active Assignment
   is the authorization coordinate for Role-bearing actions.
9. **Recovery must be conservative.** Silence, disconnection, or monitoring failure cannot prove
   that a Member is unrecoverable.
10. **Continuity is not single-process exclusion.** Runtime exclusion, exactly-once behavior, and
    responsibility continuity are separate concerns.

Role Continuity ultimately answers:

> When a Human, Agent, model, Session, or Runtime no longer continues, how can the Project still
> know what responsibilities remain, who held them during each tenure, which Work is unfinished,
> what the current situation and risks are, and how the next assignee can continue through a new
> tenure without waiting for the predecessor to come back online or provide one final summary?

## Further reading

- [Carryforth Core Model](../core-model.md)
- [Core Design: Coordinates Before Context](coordinate-and-context.md)
- [Core Design: Agent-Directed Context-Aware Project Context Retrieval](context-aware-semantic-graph-retrieval.md)
- [Core Design: Meeting](meeting.md)
- [Role Continuity Domain Contract](../../stage/role/role-continuity.md)
- [Role Continuity Implementation Design](../../stage/role/implementation-design.md)
- [Decoupling Runtime Supervisor Binding from Role Authorization](../../stage/bug/project-runtime-supervisor-binding-and-role-authorization-decoupling-fix-design.md)
- [Project Space Constitution](../project-space-constitution.md)
- [Current Status and Capability Boundaries](../current-status.md)
