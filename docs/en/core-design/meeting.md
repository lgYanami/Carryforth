# Meeting: Turning Distributed Context into Shared Conclusions and Project Outputs

> This document explains one of Carryforth's core designs: why an
> Agent-autonomous Project Space still needs Meetings, how a Meeting lets
> Humans and Agents contribute from their respective Roles, Work, and project
> experience, cross-aggregate different but related context into an actionable
> shared conclusion, and explicitly write the result back to the Project.
>
> This document describes the product mental model. It does not redefine the
> Meeting wire protocol, database state machine, or authorization protocol.
> For precise protocol semantics, see [Meeting V2](../../stage/meeting/v2/meeting-v2.md).

## 1. Core idea

> A Meeting exists so Humans and Agents can bring related information that is
> distributed across different responsibilities, work paths, and bounded
> context windows into one bounded deliberation process, form an actionable
> shared conclusion, and explicitly write the results that need to persist
> back to the Project.

The process can be summarized as follows:

```text
Related context held by each Human / Agent
  │
  ├── Role, Work, and current responsibilities
  ├── Project View / Document / Context already read
  ├── code, tool observations, and external facts
  └── different judgments, constraints, and understandings of risk
          │
          ▼
controlled Floor + canonical Speech + Directed Handoff
          │
          ▼
shared Board maintained by the moderator
          │
          ▼
frozen final Board: the actionable shared conclusion of this Meeting
          │
          ▼
ordinary domain commands + current authorization and Revision checks + canonical readback
          │
          ▼
long-lived Project state such as Work / Document / Context / Checkpoint
```

A Meeting is not a way to have more Agents repeat an answer to the same
question. It exists so that the context encountered by different participants
can matter. Nor does it merge all source material into one enormous Prompt.
The shared portion should converge as deliberation proceeds, rather than
requiring any one Agent to preload the full project history.

## 2. Why Agent autonomy still needs Meetings

An Agent can independently read project state, execute Work, maintain
Documents, and discover related material through Project Context. A
cross-domain problem, however, is often larger than the context any single
Agent can directly use at one time:

- a backend Agent understands the data model, transactions, migrations, and
  failure boundaries;
- a Desktop Agent understands interaction, user-visible state, recovery paths,
  and client constraints;
- an open-source maintenance Agent understands licenses, release surfaces,
  compatibility contracts, and contributor experience;
- a Human understands product direction, value tradeoffs, and real-world risks
  that may not yet be fully recorded in the system;
- every participant's Runtime Context may have been compressed, rebuilt, or
  moved to another process.

If one Agent alone must consolidate all material, it has to rediscover and
carry all of that context. If the project relies only on ordinary chat, the
discussion can instead scatter across message streams without stable shared
state, speaking boundaries, or an explicit conclusion.

A Meeting provides a third approach:

1. participants retain their own bounded and distinct context;
2. they formally externalize only what the current problem actually needs;
3. controlled speech exposes conflicts, gaps, and complementary information;
4. a shared Board preserves what has currently converged;
5. results that must persist are written back to the Project instead of
   remaining dependent on participants' memories.

The value of a Meeting therefore comes not from the number of Agents, but from
**complementary context, explicit deliberation, and project-owned outputs**.

## 3. Three layers of state must remain distinct

A Meeting touches three layers of state. Each has a different owner and
lifecycle.

### 3.1 Participant context

Each Human or Agent enters a Meeting from its own position:

- a stable Member identity;
- its current Role and Assignment;
- the Work it owns or is currently examining;
- project objects and Context paths it has already read;
- working material still available in its current Session;
- observations obtained through code, tools, or external systems.

This material is not automatically copied into a single meeting-wide memory.
Hidden reasoning, complete Session history, and anything that has not been
formally expressed do not become Meeting state merely because the participant
joined the Roster.

The current Agent contract supplies fresh Role Context before each complete
Turn and lets a participant perform bounded reads of Project View, Documents,
Project Context, messages, code, and related resources when needed. Semantic
path queries can also use coordinates such as Role and Work as a soft relevance
environment. The system does not automatically run a query on every Turn, nor
does it force different Agents to receive different paths merely to manufacture
variation.

Context differences should arise from the real responsibilities and work
history participants have accumulated over time, not from a temporary,
artificially partitioned "private Role knowledge base" assigned by the
Meeting. If the relevant facts are genuinely the same for several
participants, reaching the same judgment is not a failure.

### 3.2 Shared Meeting state

Material that participants formally introduce into deliberation becomes
shared state through the Meeting protocol:

- the Roster determines who can formally participate and act;
- an Intent expresses whether a participant wants to speak;
- Floor, Offer, Grant, and Handoff control the progression of speech;
- canonical Speech preserves what was formally said;
- the current Board preserves the moderator's current synthesis of the goal,
  agenda, progress, conclusions, and open questions;
- Close or Abort gives the Meeting an explicit terminal state.

Speech is the timeline of "who formally said what." The Board records "where
the group has currently arrived." Neither can replace the other.

The Board has a single moderator as its writer rather than allowing concurrent
multi-party editing. This does not assign truth to the moderator. It gives a
multi-participant deliberation one explicit convergence point so that shared
state does not lose meaning through races, duplicate summaries, or overwrites.
The current implementation persists the current / final Board and Meeting
control records, but it must not be described as guaranteeing a complete
version history of every Board replacement.

### 3.3 Long-lived Project state

Even after a conclusion appears on the Board, it is still only a shared
conclusion inside the Meeting. It becomes canonical Project state only through
the ordinary write path of the relevant domain, for example:

- creating or updating Project View objects such as Requirement, Issue, Work,
  or Resource;
- creating or revising a Project Document;
- setting Work Responsibility or creating a Commitment permitted by existing
  authorization;
- appending a Role Checkpoint;
- using a Context Document and Edge to explain the real relationship between
  the Meeting and materialized coordinates.

Project state belongs to the Project / Community, not to the moderator, a
participating Agent, or the Session that executes the writes.

## 4. "Cross-aggregation" does not mean mixing all context together

Cross-aggregating context involves three operations.

### 4.1 Expose relevant fragments

Participants only need to introduce the facts, constraints, evidence, and
judgments required to support their current contribution. Other participants
can ask for sources, request a current object read, or point out a gap, but the
Meeting does not require a complete context corpus to be assembled first.

### 4.2 Let different contexts constrain one another

The most valuable deliberation often occurs at an intersection:

- a backend design may satisfy transaction correctness while conflicting with
  the Desktop recovery experience;
- product direction may be clear but constrained by an existing compatibility
  contract or Resource capability;
- one Work item may be locally feasible while violating a release boundary
  owned by another Role;
- two Agents may use different Revisions, making an apparently shared
  conclusion rest on different facts.

A Meeting lets these differences be found, verified, and corrected in one
formal process instead of becoming hidden conflicts only after each side has
written separately to the Project.

### 4.3 Compress the result into actionable shared state

The shared Board does not copy every source fragment. It should retain what is
actually needed for the next action:

- the problem and goal;
- confirmed constraints;
- disagreements or unknowns that remain;
- the chosen approach and its applicability boundaries;
- results that must be explicitly written back;
- the blocking reason when progress cannot continue.

This is compression with provenance. Detailed facts remain readable from their
Project View objects, Documents, Meeting Speech, code, or external systems.
The Board preserves the current frontier of this deliberation.

## 5. What "consensus" means here

In this document, "consensus" means **a shared conclusion sufficient to guide
the actions that follow this Meeting**.

It does not necessarily mean:

- every participant agrees word for word;
- a vote, quorum, or multi-Human confirmation has passed;
- the system has proven the conclusion true, complete, or optimal;
- a standalone, general-purpose Project Decision object has been created;
- every risk and unknown has disappeared.

The current Meeting system has no general voting, quorum, multi-Human approval,
or Project Decision domain. The moderator decides whether the discussion has
formed a sufficiently clear final Board, or whether it should continue, abort,
or remain unresolved.

"Actionable consensus" or "shared conclusion" is therefore more precise:

```text
participant contributions need not agree completely
        +
key objections, constraints, and unknowns are explicitly recorded
        +
the moderator can form a clear final Board that can be acted on or stopped on
        =
the actionable shared conclusion of this Meeting
```

Close records the moderator's protocol declaration that the Meeting goal has
been completed. It does not automatically prove that the facts are correct,
that the Project Work is complete, or that every participant has reached
agreement in the social sense.

## 6. Humans are a key intervention point, not mandatory approvers

A Human is a first-class project member in a Meeting, not an observer outside
the system.

A Human can:

- create and moderate a Meeting;
- as a non-moderator participant in the frozen Roster, submit a Human Floor
  Request, accept or reject an Offer, speak formally, and Handoff; a Human
  moderator speaks through the moderator self-Intent and Floor-selection path;
- introduce direction, values, constraints, risk tolerance, and realities
  outside the Project into deliberation;
- complete writes through the existing Desktop, CLI, or other domain UI during
  Human-moderated Action Finalization;
- perform project governance actions outside the Meeting under existing
  Community authorization.

A Human Floor Request has a deterministic **next-seat** priority path, so a
non-moderator Human does not have to wait for the moderator's ordinary
selection. It does not revoke an already effective Grant or interrupt Speech
already in progress. After the current speech ends, the earliest request takes
the next available Floor. It may, however, preempt an ordinary Offer that has
not yet been acknowledged.

A Human is not a mandatory approver for every Meeting. The system permits
all-Agent Meetings and Agent moderators. The value of Human participation
comes from project identity, real-world information, and governance
responsibility; the "Human" label itself does not automatically confer extra
business authority.

A Community owner or admin who is not in the Roster may have emergency Abort
authority under existing rules. That does not automatically grant the right to
speak in the Meeting, moderate it, or edit its Board.

## 7. The moderator is not the Project Leader

A Meeting needs one non-transferable moderator to maintain the Board, advance
the Floor, and decide when to Close or Abort. The moderator is the temporary
convergence role for that Meeting, not the Project owner and not the Leader in
Role Continuity.

The boundary is:

| Moderator | Project Leader |
|---|---|
| Determined by Meeting Create | Established jointly by an admin Role, Community admin status, and an active Assignment |
| Authority is limited to the Meeting's Board, Floor, terminal state, and action finalization | Has the project governance authority allowed by current rules |
| May be a Human or Agent | Is the current assignee of a stable Project Role |
| Gains no Project View, Document, or Context write authority merely by moderating | Still subject to domain-specific authorization, Revision, and lifecycle rules |
| Moderation responsibility ends with the Meeting | Role and responsibility continue across Meetings, Runtimes, and Sessions |

This allows an autonomous Agent Meeting to run without requiring the Project
Leader to remain online as moderator, while avoiding an implicit path around
project governance.

## 8. Action Finalization: from shared conclusion to explicit output

Not every Meeting needs to produce business writes. Only an action-capable
Meeting enters Action Finalization when the final Board explicitly requires
action.

The exact flow is:

```text
final Board updated or confirmed unchanged
  ↓
Floor decides to enter Action Finalization
  ↓
Relay freezes discussion and the Board and creates the current Action Run
  ↓
logical moderator executes the actions decided by the Board through ordinary domain commands
  ↓
each write remains subject to its domain's identity, authorization, Revision, and lifecycle checks
  ↓
moderator canonically reads back results under the execution contract
  ↓
Human explicitly confirms, or Agent returns COMPLETE
  ↓
Harness / Desktop submits an actions-recorded ACK with the exact fence
  ↓
Relay atomically closes the Action Run and Meeting
```

Action Finalization has several important boundaries:

1. it cannot invent a second plan or new decisions outside the frozen Board;
2. neither the Board nor moderator identity grants business authority;
3. it calls the existing ordinary entry points for Project View, Document,
   Context, Role, and other domains;
4. multi-domain writes are not a cross-system transaction, so a partial success
   is not automatically rolled back by a later `BLOCK`, `RETURN_TO_BOARD`, or
   `ABORT`;
5. the Agent contract requires canonical readback, while a Human takes
   responsibility for the corresponding completion judgment through explicit
   confirmation;
6. Relay validates the current moderator identity, final Board, Action Run, and
   completion ACK control fence; it does not prove the semantic correctness of
   every business result;
7. an action that performs zero business writes because the moderator confirms
   that no write is needed can also finalize normally.

`actions-recorded` therefore means: **the moderator confirms that the actions
required by the final Board have been handled.** It is not proof of exactly-once
external execution or automatic acceptance of real-world results.

## 9. Writing context back: making the Meeting a source for future work

After a Meeting ends, it can continue to exist as a formal project record.
Once the current Community Meeting read capability is enabled, the frozen
Roster governs participation and action eligibility, while current Community
members may read Meeting records within their authorization. A Meeting is
therefore no longer merely a private Session belonging to its participants.

When materialization of the final Board creates or changes persistent
coordinates such as Requirement, Work, or Document, and there is a real
relationship worth explaining over time between those coordinates and the
Meeting, the current execution contract requires an Agent moderator, in the
same Action Finalization Turn and before returning `COMPLETE`, to:

1. complete and read back the ordinary domain writes;
2. create or revise an ordinary Project Document that explains the reason,
   impact, and boundary of the relationship;
3. attach the current Meeting and the coordinates actually materialized to the
   same exact Context Edge;
4. canonically read back that Edge.

A Human moderator can explicitly perform the same maintenance through the
existing Project Context entry points and takes responsibility for that
judgment when confirming action completion. Relay does not uniformly collect
these business readbacks for the Human path, and it does not infer that Context
was written back solely from a completion ACK.

```text
Meeting M
  + Work W
  + Document D
        │
        └── exact Project Context Edge
                 │
                 └── Context Document:
                     explains how W / D arose from the shared conclusion in M,
                     and the applicability boundary of that conclusion
```

This step is always explicit. The system does not automatically create an Edge
merely because Speech, a final Board, Close, or business writes exist. If there
is no real cross-coordinate relationship to explain, it should not fabricate a
Context Document merely for formal completeness.

Attaching a Meeting coordinate also requires validation by the source domain.
A verified terminal Meeting can serve as a stable coordinate. An active Meeting
in `finalizing_actions` can be used only when the current Action Run, frozen
Board, control fence, and other conditions match completely. A client-declared
phase or summary cannot substitute for those checks.

## 10. Why results do not depend on a single Session

Meeting correctness and continuity come from:

- a stable Meeting identity;
- canonical roster, Board, State, Floor, and Action Run held by Relay;
- the stable Member identity of the logical moderator;
- exact Board / run / window fences;
- the ordinary business domains' own authorization and Revision checks.

They do not depend on:

- one model process remaining alive;
- a fixed physical worker slot;
- reusing the same ACP Session from beginning to end;
- the moderator Agent's hidden history remaining in the context window;
- the Project Leader staying online.

The system prefers to reuse an existing Meeting Session for better local
context continuity. If a slot, process, or Session is replaced, however, the
new Action Turn must reconstruct its complete input from the frozen Board,
canonical Meeting envelope, current Role Context, and Action fence. A physical
Session is not a source of authorization or correctness.

This is a concrete expression of "continuity belongs to the Project": temporary
executors can change, while canonical Meeting state and explicitly materialized
project results remain verifiable, readable, and actionable by later Humans
and Agents.

## 11. End-to-end example

Suppose the Project is discussing "how to enable semantic queries while
preserving local data safety."

### 11.1 Different participants bring different context

- the Relay Agent brings the query gate, signatures, Provider egress, and
  database resource boundaries;
- the Desktop Agent brings timeout UX, retry behavior, error pages, and user
  interaction paths;
- the open-source maintenance Agent brings `.env.example`, startup scripts,
  public documentation, and compatibility identifiers;
- the Human brings the current direction constraint: support only one local
  Relay for now and do not target production yet.

These materials are related, but no participant has to carry all of them in
advance.

### 11.2 The Meeting forms a shared conclusion

Through Speech, follow-up questions, and Handoff, participants discover that:

- not every safety gate can be removed for local convenience;
- the current single-Relay deployment does not need to continuously maintain a
  multi-Pod Fleet lease;
- Provider configuration, the Community query gate, and confirmation that the
  problem leaves the local control plane must remain;
- Desktop should distinguish temporary unavailability from permanent lack of
  support.

The moderator organizes this material into the final Board and retains the
unfinished qualification boundary for production multi-instance deployment.

### 11.3 Results enter the Project

If the final Board requires implementation and the Meeting enters Action
Finalization, only the Meeting's immutable logical moderator may execute this
Action Run and complete its `actions-recorded` ACK. The moderator must still
hold the authorization required by every business operation, for example to:

- update configuration and Relay implementation;
- update an operations Document;
- create or update Work;
- use a Context Document to explain the relationship among this Meeting, the
  configuration Work, and the operations Document;
- read back all canonical results and confirm completion of the actions.

Other authorized members may independently perform ordinary project writes
outside the Meeting, but they cannot substitute for the moderator in the
current Action Run or submit its completion ACK on the moderator's behalf.

Even if the moderator Agent later exits, the Project Leader changes, or the
original ACP Session disappears, a new member can still understand the
conclusion and its provenance through Project View, Document, the Context Edge,
and the Meeting record.

## 12. What a Meeting is not

| A Meeting is not | Why |
|---|---|
| Ordinary group chat | It has a fixed Roster, controlled Floor, canonical Speech, a shared Board, and an explicit terminal state |
| A merger of multiple Agents' hidden reasoning | Only formally externalized contributions enter the Meeting; shared state contains no private reasoning |
| An automatic context partitioner | Context differences arise from real Roles, Work, and experience and should not be manufactured by the system |
| A voting or unanimous-consent system | There is currently no quorum, voting, or multi-Human confirmation protocol |
| A general-purpose Project Decision domain | Shared conclusions first exist on the Board and must be expressed explicitly through existing domain objects |
| A Project Leader command | The moderator converges the Meeting but gains no project governance or business authorization as a result |
| An automatic business workflow | Action Finalization calls ordinary business entry points; it neither proxies nor rolls them back |
| An authorization amplifier | Roster, Board, Handoff, similar paths, and moderator identity never expand existing authorization |
| Proof of correctness | Close, signatures, and ACK prove protocol state and bindings, not that a conclusion is true or optimal |
| A permanently running Agent team | A Meeting is a temporary collaboration lifecycle with a goal and a terminal state |

## 13. Design principles derived from this model

1. **Differences come from real responsibility; the Meeting does not invent
   them.** Roles, Work, and project experience shape the paths participants
   attend to.
2. **A shared frontier is more important than merging all context.** A Meeting
   aggregates only what the current problem actually needs.
3. **Formal externalization is more durable than implicit memory.** Only
   Speech, the Board, and explicit project writes can reliably survive across
   Sessions.
4. **A single convergence point does not imply single ownership.** The
   moderator maintains shared state, but the result belongs to the Meeting and
   Project.
5. **A shared conclusion is not automatic truth.** Objections, unknowns,
   evidence, and boundaries must remain preservable and readable.
6. **Discussion and action are separate.** External systems are read-only
   during discussion; actions occur explicitly under a frozen Board and
   existing business authorization.
7. **Meeting output and Project output are separate.** The Board is the
   Meeting's conclusion; domain writes become the Project's current state.
8. **Context writeback must be real and explicit.** Relationships are not
   inferred automatically from Meeting text or write effects.
9. **Logical identity takes precedence over physical Session.** Worker slots
   and model sessions may change; Relay fences and project state preserve
   continuity.
10. **A Human is a key participant, not a universal approver.** Humans can
    intervene in direction and constraints while remaining subject to project
    identity and authorization.

Ultimately, a Meeting does not solve "how to make Agents hold meetings." It
solves this problem:

> When the context needed for a project problem is distributed across
> different Humans, Agents, Roles, Work, and bounded Runtimes, how can those
> contexts interact in a governable process, form an actionable shared
> conclusion, and return the results that must persist to the Project?

## Further reading

- [Carryforth core model](../core-model.md)
- [Core design: Role Continuity](role-continuity.md)
- [Core design: Coordinates before context](coordinate-and-context.md)
- [Core design: Context-aware semantic graph retrieval](context-aware-semantic-graph-retrieval.md)
- [Meeting V2](../../stage/meeting/v2/meeting-v2.md)
- [Meeting Action Finalization](../../stage/meeting/fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)
- [Maintaining Project Context during Meeting Action Finalization](../../stage/project-context/meeting-action-finalization-context-write-implementation-design.md)
- [Project Context domain specification](../../stage/project-context/project-context.md)
- [Project Space constitution](../project-space-constitution.md)
- [Current status and capability boundaries](../current-status.md)
