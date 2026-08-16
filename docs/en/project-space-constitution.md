# Carryforth Project Space Constitution

> Status: Current design and implementation baseline
>
> Alignment date: 2026-08-13
>
> Higher-level positioning: [Project Positioning and Goals](project-positioning.md)
>
> Model guide: [Carryforth Core Model](core-model.md)
>
> Capability status: [Current Status and Capability Boundaries](current-status.md)

## 1. Purpose and scope

This document defines the high-level governance principles currently shared across the Carryforth
Project space and explains how those principles map to the implemented Project View v3, Role
Continuity, Project Documents, Project Context, Meeting, messaging, signatures, and Community
permission model.

This document answers:

- which state should be held by the Project and which state may remain local to a Member;
- how Human, Agent, Role, Assignment, and Runtime differ;
- within which boundaries an Agent may act autonomously and which governance responsibilities
  remain with Humans;
- how conversations, Documents, Context relationships, Meeting outcomes, and external actions
  enter the Project;
- which read results are canonical state and which are derived views with provenance;
- which general governance objects the current implementation does not provide, and therefore
  which capabilities cannot be inferred from older concepts.

This document is not a wire schema, database schema, permission matrix, deployment manual, or
production qualification statement. Precise object relationships, signatures, Revisions, state
machines, and error semantics are defined by the corresponding domain specifications and current
code. Availability also depends on capabilities, durable gates, Member permissions, and runtime
readiness.

### 1.1 Two kinds of constraints

This document contains two kinds of material:

- **Constitutional principles**: high-level constraints that product design, implementation, and
  collaboration should not violate;
- **Current contracts**: boundaries currently adopted and verified by the Relay, database, CLI,
  or Desktop.

Unless explicitly identified as a "current contract," the word "must" in this document first
expresses a governance and design requirement. It does not claim that the code already enforces a
general policy engine automatically. The existence of code also does not mean that a feature is
enabled or production-qualified.

### 1.2 Normative terms

- **Must**: a boundary that cannot be bypassed silently. A violating result cannot be described as
  conforming to Carryforth Project governance.
- **Should**: the current default. A departure requires an explicit reason, impact assessment, and
  follow-up.
- **May**: permitted by this constitution, but does not automatically create authority, state, or
  a product commitment.

## 2. Current Project model

Carryforth's core principle is:

> Continuity belongs to the Project, not to any one Agent.

In the current implementation, one Carryforth Community forms one Project's root identity, Member
admission, authorization, and data boundary. Community is the current technical boundary; Project
is the long-lived product and collaboration meaning it carries.

```text
Project / Community
│
├── Project View          Current first-order Project state
├── Role Continuity       Stable responsibility and replaceable executors
├── Project Documents     Stable identity and immutable Revisions
├── Project Context       Explicitly preserved second-order relationships
├── Meetings              Bounded, structured collaboration
├── Channels / Messages   Signed day-to-day collaboration records
└── Members               Stable Human and Agent identities
```

A Project is not a code repository, Agent Session, Leader, team process, or the Git collaboration
preview surface named **Projects** in Desktop. A Project may relate to multiple repositories and
external systems of record, which continue to own their respective authoritative facts.

## 3. Article One: the Project owns continuity

### 3.1 Members may change; the Project must continue

Humans, Agents, Leaders, models, Providers, Personas, Sessions, and Runtimes may join, leave, stop,
or be replaced. No one Member may become the sole holder of Project identity, Project memory, or
the authority to interpret the Project.

A new Member should be able to resume work from authorized canonical state, the current Revision,
Work, Checkpoints, Handoffs, Documents, and Context, rather than depend on the old Runtime remaining
online or providing one final summary.

A Handoff is an important transition entry point, but not a prerequisite for continuity. Without a
Handoff, the Project must still support recovery from its own canonical state. A later Member must
explicitly continue responsibility through a new Assignment and Commitment.

### 3.2 The disappearing-Member test

When deciding whether information may remain local to a Member, ask:

> If this Member permanently disappeared now, would losing this information prevent the Project
> from continuing, cause other Members to form an incorrect judgment, erase a commitment, or hide
> an important risk?

If the answer is yes, the Member must write the minimally sufficient information back to a
currently supported Project surface. Writeback does not require disclosure of complete
conversations, drafts, prompts, model reasoning, or working process.

### 3.3 Relay is the current canonical boundary

Desktop, `cf`, Managed Agents, and other clients cannot each maintain a competing version of
Project truth. In the currently supported architecture, the Relay verifies and persists
Community-scoped canonical state. Client caches, model-generated summaries, and local views are
derived reads only. Source-owned `summary` fields on canonical objects such as Project View and
Meeting are not the derived summaries described here.

"Project-centered" does not mean hiding a central Agent that understands and schedules everything.
The Relay is a state and authorization boundary, not a super-Agent, and it does not monopolize
expert judgment.

## 4. Article Two: canonical state, derived views, and external facts must remain separate

### 4.1 Current canonical state

The main current canonical surfaces include:

- Project View v3 Project Profile, Goal, Role, Plan, Stage, Requirement, Issue, Work, and Resource;
- Role Proposal, Assignment, Work Responsibility, Work Commitment, Checkpoint, and Handoff;
- stable Project Document identity, Revision, current head, and tombstone;
- Project Context Edges / Hyperedges and their Context Document bindings;
- verified Meeting state such as roster, Board, Floor, Speech, Handoff, close, and abort;
- signed messages and necessary governance records within Community / Channel scope.

These objects do not form a state machine in which arbitrary facts can be inferred from each
other. Canonical state changes only when the corresponding domain command executes explicitly and
passes signature, permission, Revision, lifecycle, and Community validation.

### 4.2 A derived view is not a second source of truth

Role Briefs, client- or model-generated summaries, search indexes, graph layouts, semantic paths,
health or readiness readings, and UI caches are derived reads. This does not include source-owned
`summary` fields maintained by canonical Project View, Meeting, and similar objects. A derived read
must retain its sources, Revision, currentness, or applicability boundary. It cannot override a
canonical object when they conflict, nor can it create authority, responsibility, a Project Context
Edge, or a Project decision.

Relay-returned semantic candidates and complete-path results are signed and bound to their exact
requests. Canonical read descriptors or commands derived after verification are navigation aids,
not additional signed Project facts. An Agent still reads current authoritative objects through
their owning surfaces before relying on content.

### 4.3 External facts remain with external systems of record

Code repositories, design tools, issue trackers, deployment platforms, customer systems, and
other external resources continue to hold their own real-world state. Carryforth may preserve a
Resource, Guide, stable coordinate, version, observation, or Project meaning, but registering a
reference does not:

- copy external authority;
- automatically install or execute anything, or obtain a secret;
- prove that an external action occurred;
- interpret tool success as completion of a business objective.

## 5. Article Three: state that affects the Project must be written back minimally and sufficiently

### 5.1 When writeback is required

Information or behavior should be written back when it begins to affect other Members, future
actions, or Project continuity. Examples include:

- committing to Work, changing progress, or discovering an inability to continue;
- discovering an important fact, unknown, risk, Issue, or conflict that changes Project judgment;
- creating a dependency, constraint, or external commitment that affects others;
- forming a design, explanation, rationale, or applicability boundary that later Members need to
  inherit;
- preparing to modify a shared object or invoke a capability with potentially material, external,
  concurrent, or irreversible effects;
- pausing, handing off, requesting replacement, or leaving a responsibility;
- turning a result from a Meeting or message into canonical Project state.

### 5.2 Minimally sufficient writeback

When applicable, writeback should include:

- the conclusion, intent, or state change;
- its scope and where it does not apply;
- necessary rationale and key assumptions;
- risks, uncertainty, and known disagreement;
- responsibility, next steps, and readback coordinates.

Writeback must use existing domain objects rather than place everything into one free-form
"memory." For example, directly current state belongs in Project View, long-form content in a
Document, second-order relationships in Project Context, responsibility context in a Checkpoint /
Handoff, formal collaboration in a Meeting, and day-to-day coordination in a Channel / Message.

### 5.3 State that may remain local to a Member

Drafts, hypotheses, exploratory branches, prompts, internal model analysis, temporary tool
processes, and personal presentation preferences may remain local while they have no Project
impact. Once they become a basis for Project actions, commitments, or risk judgments, there must be
a Project record sufficient for later verification and continuation.

A Member may think privately, but cannot constrain the Project privately.

## 6. Article Four: identity, Member, Role, Assignment, and Runtime must remain separate

### 6.1 Stable identity

Humans and Agents become Community Members through stable public-key identities. Restarting a
Runtime with the same public key still represents the same Member. Changing the public key creates
a new Member, which cannot impersonate its predecessor or rewrite the predecessor's historical
contributions.

Connecting to the Relay, joining a Channel, starting a process, configuring a model, or holding
external credentials does not automatically establish Community membership, a Role Assignment, or
business authority.

### 6.2 Role and Assignment

A Role is a long-lived responsibility position. An Assignment is a bounded tenure in which one
Member holds that Role. Current contracts include:

- one Role has at most one active Assignment at a time;
- one Member has at most one active Assignment at a time;
- a Proposal atomically activates or replaces an Assignment only after the candidate accepts and
  an authorized governor approves;
- Assignment tenures preserve history and cannot be reused or rewritten by later assignees;
- an assignee cannot unilaterally end its own Assignment and may only request replacement or report
  that it cannot continue;
- stopping or disconnecting a Runtime, or changing its model, does not automatically end an
  Assignment.

Ending or replacing an Assignment still requires current governance authority. The Community
owner may govern it. An Active Leader may govern only `member` Assignments and cannot end its own
Assignment or that of a peer Leader. A verified human owner may end the Assignment of a managed
Agent it owns, even when that Agent currently holds a Leader Role. An ordinary Role cannot govern
Assignments. Automatic `unrecoverable` applies only to supervised Runtimes that meet objective
unrecoverable conditions; silence is not proof.

A Community Member may have no Assignment. A Member with both Community membership and an active
Assignment is currently often called an active Project Member, but Assignment is not a prerequisite
ACL for every ordinary Project content read or write.

### 6.3 Runtime and provenance attribution

A Runtime is a short-lived execution instance of an Agent. Runtime supervision, bindings, leases,
and fences support operational evidence, leasing, recovery, maintenance coordination, or explicit
provenance attribution. They are not a source of business authority.

The current contract does not guarantee only one writable Runtime for an Assignment at a time. As
long as an old process still holds the Member private key and the Assignment remains active, it may
still submit Role-bearing commands. The actual hard revocation coordinates are Assignment,
Community authority, and bans. Write concurrency continues to rely on Revision / CAS and
append-only history.

When a command explicitly carries Runtime attribution, the current contract requires exact
validation. When it does not, missing Runtime attribution neither grants nor revokes
otherwise-valid business authority and cannot replace Community, Assignment, or domain permission
checks.

### 6.4 Current authorization layers

Whether an operation is allowed must be validated independently across the relevant surfaces:

1. the trusted host / workspace binding to a Community;
2. Community membership, bans, and basic owner / admin / member authority;
3. domain capabilities, durable gates, and runtime readiness;
4. the exact active Assignment when the operation represents a Role;
5. object Revision, generation, lifecycle, and reference currentness;
6. caller signature and any required exact request binding;
7. Runtime attribution when explicitly supplied.

Client event tags, UI switches, tool reachability, shared credentials, semantic relevance, and an
Agent's retrieval context cannot create or expand authority. A Community must be resolved from a
trusted host / workspace, not from client-controlled tags.

## 7. Article Five: responsibility continues through Role and Work

### 7.1 Governance root and Leader

The current Role Continuity contract treats the unique Community owner as the Human governance
root. Owner authority is not granted by a Role, and the owner need not hold an imaginary "Owner
Role."

A Leader is a `level=admin` Role. An Active Leader must have both Community admin status and the
exact active admin Assignment. Multiple Leaders may currently exist, but there is no domain-level
Leader permission isolation. A Role's textual responsibilities must not be mistaken for a
fine-grained ACL that the technology already enforces.

The owner or an Active Leader may govern a `member` Role. Only the owner may create an admin Role
or change its level or lifecycle. An ordinary Member may request a Role and act on its own Proposal,
but cannot change the Role definition directly.

### 7.2 Work, responsibility, and Commitment

Long-lived responsibility for Work is anchored to the responsible Role. Concrete acceptance is
expressed by a Work Commitment from that Role's current assignee. Current contracts include:

- one Work item has at most one responsible Role;
- one Work item has at most one active Commitment;
- the Commitment's Assignment Role must match the responsible Role;
- ending an Assignment or Commitment does not automatically complete, cancel, reassign, or rewrite
  Work;
- a successor must explicitly continue unfinished Work through its own Assignment and a new
  Commitment.

Setting or clearing the responsible Role is a governance action that only the Community owner or
an Active Leader may perform. An assignee may accept or release Work only through a Commitment
matching its exact active Assignment and cannot bypass responsibility governance through an
ordinary Work edit.

Checkpoints and Handoffs are append-only continuity records, not copies of Project View, Documents,
Issues, or Work. The system does not turn one summary into a new global source of truth.

### 7.3 A Leader is replaceable

A Leader may coordinate priorities, responsibility gaps, and cross-Member collaboration, but is
not a parent process for other Agents, the owner of Project Context, or a prerequisite for the
Project to continue. In Carryforth, A2A means that Agents collaborate by attaching to the same
Project. It does not promise a particular direct peer-to-peer protocol. Agents primarily share
state through the Relay, Channels, Project objects, Meetings, and `cf`. ACP connects only a Managed
Runtime to its harness / client; it is not an Agent-to-Agent protocol.

## 8. Article Six: Agents are autonomous within authority; Humans retain governance responsibility

### 8.1 Agent autonomy boundary

An Agent is a first-class Project Member. Within verified Community, Role, Assignment, capability,
object-lifecycle, and risk boundaries, it may discover, propose, claim, split, execute, and hand
off work.

Agent autonomy is a governance principle, not a general policy engine already present in the
system. A callable tool, model-generated output, running Runtime, or multiple Agents reaching the
same answer cannot replace authorization, evidence, or Project writeback.

### 8.2 Governance responsibilities retained by Humans

Human governors remain responsible for:

- Project purpose, scope, and value boundaries;
- owner-level permissions, Member admission, and material authorization;
- legal, ethical, security, privacy, commercial, and ultimate responsibility;
- acceptance of material, irreversible, or long-lived external risk;
- value conflicts that cannot be resolved by factual verification alone.

Human governance does not mean every Human automatically has every permission, nor does it allow a
Human to write an unverified preference as fact. Authority remains expressed through the owner,
Community level, active Assignment, signatures, and specific domain contracts.

### 8.3 Urgency does not expand authority

There is currently no general Emergency State object or automatic privilege-escalation engine. Any
Member may stop its own actions, report risk, request help, or trigger an existing disable /
fail-closed path. An "emergency" label does not grant new authority to read or modify another
Member's state or operate an external system. Further action still requires existing authority and
an appropriate record.

## 9. Article Seven: Project View expresses only explicitly registered first-order state

Project View v3 currently contains nine stable object types:

- Project Profile;
- Goal;
- Role;
- Plan;
- Stage;
- Requirement;
- Issue;
- Work;
- Resource.

It answers what the Project is, what it aims to achieve, how it plans to proceed, where it is,
and which responsibilities, requirements, problems, work, and resources exist. It does not
automatically explain every cause, impact, or implicit relationship.

Project View relationships have precise cardinality and Revision contracts. For example, Work
must handle one Requirement or Issue; a Stage belongs to a Plan; and an Issue's `about` reference,
planning position, and handling Work are separate dimensions. No relationship may become true
automatically because of a title, textual similarity, or client guess.

Implicit cascading between objects is forbidden. Changing a Plan / Stage relationship does not
automatically change Requirement, Issue, or Work state. Completing Work does not automatically
complete a Requirement, Goal, or Project. Deletion, replacement, and revision must follow the
corresponding lifecycle.

See [Project View](../stage/project-view/project-view.md) and the
[Project View Object Relationship Design](../stage/project-view/object-relation-design.md) for the
complete contracts.

## 10. Article Eight: Document, Resource, and Context respectively carry content, entry points, and relationships

### 10.1 Project Document

A Project Document has a stable `document_id`, immutable full Revisions, and an explicit
current Revision. Concurrent conflicts must not overwrite silently. Deletion is represented by a
verifiable tombstone.

A Document may carry designs, constraints, explanations, Meeting outcomes, and Context meaning,
but is not a storage location for API Keys, private keys, or credentials. See the complete
[Project Document contract](../stage/document/document.md).

### 10.2 Resource and Guide

A Resource represents the coordinate of an asset or capability associated with the Project, not
the resource itself. A current v3 Resource consists of a name, an open `resource_kind`, an optional
summary, and a required `guide_document_id`. That field must reference an active Project Document
that serves as the Resource's Guide.

Registering a Resource does not grant access, download content, install a tool, read a secret, or
execute a command.

### 10.3 Project View Context Reference

An active Project View object other than a Resource may reference a Resource, or a live / pinned
Revision of a Document. A Resource itself may reference only a Document, not another Resource.

This kind of Context Reference is a lightweight reference to a related asset held directly by the
object. It is not a Project Context Edge and does not automatically create an Edge, copy content,
or grant authority.

### 10.4 Project Context Edge

Project Context explicitly preserves the second-order semantics of "why these objects are
related." The current model is an undirected Edge / Hyperedge with:

- at least two Project View, Document, or Meeting coordinates in the same Community;
- one or more ordinary Project Documents bound as Context Documents that explain the
  relationship;
- at most one Edge for the same exact normalized coordinate set in a Project;
- at most one Edge to which a Document belongs as a Context Document;
- at least one Context Document retained by an active Edge, with the active Edge disappearing
  after its final Context Document is detached;
- no silent shrinking or deletion of an existing Edge when a coordinate becomes a tombstone.

The system does not infer or create Edges automatically from conversation, titles, or textual
similarity. See the complete [Project Context contract](../stage/project-context/project-context.md).

### 10.5 Agent-directed context-aware graph retrieval

A Managed Agent may use its current verified Role and relevant Work, Issue, Meeting purpose, or
other task facts to progressively retrieve Project Context. It prefers a reliable Coordinate from
current work, uses semantic discovery only when no start is known, then chooses each
`Coordinate → Edge → Coordinate` hop from bounded semantic candidates, lightweight canonical
observations, and relation Documents. Scores order observations; the Agent makes the selection.

This derived read uses the one Project-owned Context Graph. It does not create private Agent/Role
graphs, infer Edges, rewrite relationships, or expand authority. Natural-language semantic
operations require an external Provider, index generation, Community gates, Member authorization,
and confirmation that query text may leave the system.

The bounded complete-path `semantic-query` remains available as a supplementary query surface. Its
soft query context may affect recall and ranking, but it is not the primary Managed Agent retrieval
workflow and is not a filter, permission, action gate, or guarantee of semantic correctness.

## 11. Article Nine: messages and Meetings must materialize outcomes explicitly

### 11.1 A message does not automatically become Project state

Channels / Messages are signed collaboration records with Community / Channel boundaries, but a
conversation, agreement expressed by multiple Members, model summary, or delivered message does
not automatically modify Project View, a Document, Context, or Work.

A conclusion that affects the Project's future must be explicitly written back by an authorized
Member through the corresponding domain command. The system does not require complete
conversations, drafts, or internal model reasoning to be preserved.

### 11.2 Current Meeting positioning

Meeting V2 is a bounded, structured collaboration object. It is not a Project-creation ritual, a
constitutional court, the only Human entry point, or a mandatory approval step for every Project
choice.

The current model uses a fixed roster, makes the initiator the moderator, and includes Board,
Floor, Speech, handoff, lease, timeout, close, and abort state. Humans and Agents may both moderate
or participate, and all-Agent Meetings are allowed. The roster controls participation and action
eligibility; Community read capability is governed by a separate gate and permission contract.

There is currently no general Meeting-type template, quorum, voting, multi-Human confirmation,
dynamic roster, moderator transfer, "founding Meeting," or "constitution-amendment Meeting"
protocol.

### 11.3 A Meeting does not automatically establish a Project decision

The Board is the moderator-maintained current synthesis, Speech and handoff record the
collaboration process, and Close / Abort express the Meeting lifecycle. This content does not by
itself establish a general Project Decision or modify Project View.

A Meeting outcome becomes Work, a Document, Context, a Checkpoint, or other canonical state only
after an authorized Member explicitly writes it through an existing business surface or ordinary
domain command and reads it back. For an action-capable Meeting, Action Finalization is only a
bounded phase in which the moderator performs those ordinary business operations and submits an
`actions-recorded` ACK. The Meeting does not proxy business writes or verify the semantics of
external results. Moderator authority is not write authority over business objects.

See the complete [Meeting V2 model](../stage/meeting/v2/meeting-v2.md).

## 12. Article Ten: there is currently no independent general Decision domain

Observations, hypotheses, suggestions, proposals, conversational consensus, Meeting Boards, Close,
and tool results must remain separate from canonical Project changes.

The current implementation has no general Project Decision object and state machine covering
"candidate decision—established—effective—suspended—superseded—repealed." Text cannot be labeled
"Decision" and then claimed to have system-level binding force.

When a choice affects the Project, Members should:

1. preserve the necessary rationale in a Document, Issue, Requirement, Work, Context, Checkpoint,
   or Meeting;
2. have an authorized Member perform the actual required domain state change;
3. make changes to external systems in their external systems of record and read them back;
4. explicitly distinguish discussion records, rationale, authorization actions, and actual
   effects.

Binding force comes from real authority, domain objects, signatures, and external authoritative
state, not from a prose label. A future independent Decision domain must undergo separate design,
protocol, migration, permission, and compatibility review.

## 13. Article Eleven: action, verification, and Work state must not impersonate one another

Current Work uses direct lifecycle state. It does not implement the general chain of
ExecutionAttempt, ResultSubmission, Verification, Acceptance, and CompletionRecognition objects
described in an older version of this constitution.

Even so, every Member must follow these principles:

- intent to act does not mean a command was sent;
- tool success does not mean an external effect occurred;
- the existence of an output does not mean a Requirement was satisfied or an Issue resolved;
- passing tests does not mean the result was adopted, deployed, or had its risk accepted;
- marking Work complete does not automatically close a parent object, dependency, risk, or
  external side effect;
- timeout, cancellation, and connection failure do not prove that an external action had no
  partial effect;
- when a retry may produce side effects, reconcile first or use a reliable idempotency boundary.

An action with Project impact should write the necessary intent, result, evidence, limitations,
and follow-up responsibility back to currently supported objects. A future, more complete
execution—verification—acceptance model must not rewrite existing facts through implicit cascades.

## 14. Article Twelve: authority, visibility, and secrets must be represented honestly

### 14.1 Current visibility boundaries

The current implementation primarily provides:

- Community-scoped Project View, Document, and Context surfaces;
- messages scoped to Channel membership;
- Meeting surfaces controlled by the Meeting roster and a separate Community read gate;
- Member-local temporary state that does not yet affect the Project.

There is currently no universal, fine-grained "Project-restricted state" ACL for every object and
field, nor a framework that automatically produces redacted dependency signals or
"minimally sufficient Context proofs." Documentation must not describe these future designs as
current capabilities.

Shared within a Project does not mean visible on the public internet; not visible does not mean an
object does not exist. A client must distinguish lack of permission, a disabled capability, a
temporarily unavailable dependency, a nonexistent object, and a data conflict. It must not conceal
these boundaries behind an empty result.

### 14.2 Secrets do not enter Project content

API Keys, private keys, real credentials, access tokens, and sensitive content that should not be
shared must not be written to Project Documents, Project View, Context, Messages, Meetings, logs,
or test fixtures. They must remain in controlled secrets storage, a keyring, or a private local
environment.

### 14.3 Fail closed across Projects

Canonical objects, Role Continuity entities, Documents, Context coordinates, Meetings, and
references must belong to the same Community. Cross-Project reads, references, attribution, or
writes must fail closed. Client tags or textual content cannot redirect the tenant.

## 15. Article Thirteen: conflicts, revisions, and history must not be erased silently

### 15.1 Distinguish facts, judgments, and disagreements

A shared baseline does not mean enforced consensus. Members must distinguish facts, sources,
assumptions, suggestions, unknowns, and disagreements. Multiple Agents repeating the same
conclusion does not automatically create multiple independent pieces of evidence.

When a conflict is discovered, the exact object, Revision, scope, evidence, and affected actions
should be preserved. Important ambiguities that cannot be resolved promptly should remain visible
and may be handled through an Issue, Document, Meeting, or later Work. A summary or model must not
silently choose a side.

### 15.2 Corrections create new history

Signed events, Document Revisions, Assignment tenures, Checkpoints, Handoffs, and other canonical
records that have become a basis for the Project must not be rewritten without a trace. A
correction, supersession, tombstone, recovery, or compensation should create new traceable state
and retain the effects the original record once produced.

Optimistic concurrency conflicts must be returned explicitly. The system must not overwrite
automatically or treat an old Revision as current state.

### 15.3 Honest limits of auditability

The current audit hash chain detects inconsistency and tampering within the chain. It is not an
external immutable ledger, a non-repudiation proof, or a compliance certification. An attacker
with direct database write access may recompute an unkeyed hash chain. Some audit paths are
asynchronous best-effort and cannot support a claim that every external effect is recorded
atomically with its domain transaction.

Historical trust therefore comes from the combination of signatures, Revisions, domain
constraints, readback, and audit evidence, not from a single "audited" label.

## 16. Article Fourteen: documentation cannot bypass capability gates or maturity

The following facts must always remain separate:

- code exists;
- a process has started;
- a Desktop preview switch is on;
- the Relay advertises a capability;
- a Community durable gate is enabled;
- the current Member is authorized;
- data and dependencies are ready;
- the feature has completed local, release, or production qualification.

`./start.sh` or `just start` only establishes a local source-development stack. It does not replace
the owner or operator in initializing Project View, opening a Community gate, confirming Provider
data egress, or granting Member authority.

The current repository remains under active development and is preparing its first public source
snapshot. It is intended only for local source builds, evaluation, and reference learning; public
source is not a versioned release or a packaged artifact. Semantic retrieval, Meeting, Git Projects,
and some Desktop surfaces remain subject to
preview or qualification boundaries. This document cannot be used to claim production readiness,
multi-instance safety, stable upgrades, or platform support.

## 17. General governance models explicitly not implemented today

An older version of this constitution described the following concepts as established Project
institutions:

- a general Project Decision and state machine for establishment, effect, suspension,
  supersession, and repeal;
- Context Requirement, Dynamic Context View, Context Gap, and sufficiency proof;
- Execution Attempt, Result Submission, Acceptance, and independent Completion Recognition;
- Continuity Assurance Requirement, Coverage, Evaluation, Gap, and Project Health View;
- a general Emergency State and automatic privilege escalation;
- field-level restricted state and redacted dependency signals for every Project object;
- Meeting templates, quorum, voting, multi-person confirmation, and protocols for founding a
  Project or amending the constitution.

Some of these concepts may remain future research directions, but they are not current canonical
objects, sources of authority, or product guarantees. Any later implementation must be introduced
through independent domain design, threat modeling, protocol, migration, compatibility, and
qualification. It cannot be restored as a "current capability" merely because an older version of
this document mentioned it.

## 18. Interpretation, amendment, and documentation hierarchy

### 18.1 Documentation hierarchy

- [Project Positioning and Goals](project-positioning.md) explains why Carryforth exists and what
  it aims to become;
- this constitution defines the current Project governance boundaries that cannot be crossed
  silently;
- the [Core Model](core-model.md) explains current objects and relationships;
- domain specifications define precise protocols, state machines, permissions, and lifecycles;
- [Current Status](current-status.md) explains which capabilities are implemented, require
  activation, remain under qualification, or are not yet committed;
- code, migrations, schema, and tests are the current executable contract.

When documentation and implementation conflict, the more permissive side must not be selected to
expand authority. Record the inconsistency, fail closed at the safety boundary, and bring the
documentation, code, and tests back into alignment in the same revision.

### 18.2 Constitutional amendment

There is currently no dedicated "constitution Meeting" or on-chain amendment protocol. Changes to
this constitution use the repository's normal review and change process and must:

- explain the reason, scope, and compatibility impact;
- distinguish governance principles, current implementation, and future design;
- update affected domain specifications, Chinese guides, status documents, and tests together;
- not silently expand authority, open a gate, or rewrite existing history through wording changes.

### 18.3 Supersession by this revision

This revision supersedes descriptions of future governance systems in the "first edition
consensus" that conflict with the current implementation. The core principles it retained—Project
ownership of continuity, impact-triggered writeback, separation of identity and Runtime, Agent
autonomy within authority, Human governance responsibility, explicit state change, and traceable
history—remain valid and are carried by current domain objects.

## 19. Reference specifications

- [Project Positioning and Goals](project-positioning.md)
- [Carryforth Core Model](core-model.md)
- [System Overview](system-overview.md)
- [Current Status and Capability Boundaries](current-status.md)
- [Project View](../stage/project-view/project-view.md)
- [Project View Object Relationship Design](../stage/project-view/object-relation-design.md)
- [Role Continuity](../stage/role/role-continuity.md)
- [Project Document](../stage/document/document.md)
- [Project Context](../stage/project-context/project-context.md)
- [Meeting V2](../stage/meeting/v2/meeting-v2.md)
- [Semantic pgvector Operations](../semantic-pgvector-operations.md)
- [ARCHITECTURE.md](../../ARCHITECTURE.md)
- [SECURITY.md](../../SECURITY.md)
