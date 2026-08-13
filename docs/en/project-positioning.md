# Project Positioning and Goals

> This document records the project positioning, goals, and principles on which consensus has
> currently formed. It answers “why does this project exist, what should it become, and what form of
> collaboration should it support?” It does not discuss specific technical implementations or
> prescribe the detailed governance system of the project space in advance.

## 1. Background

As Agents increasingly participate in or perform requirements analysis, development, testing, and
other work, Humans still have to repeatedly supply different Agents with project context scattered
across product documents, designs, code repositories, historical tasks, and project members’
experience.

Existing forms of collaboration usually bind continuity to a particular Agent or one execution of a
team:

- In a primary Agent–SubAgent model, the primary Agent selects context, decomposes and dispatches
  work, and each SubAgent returns a result after completing a task within its own lifecycle.
- In an Agent Team organized by a Leader, members can have independent context and collaborate with
  one another, but the team’s creation, current situation, and continuation can still readily depend
  on the Leader or team instance.
- When an Agent, Session, or team ends, much of the project knowledge, decision rationale, work
  state, and experience cannot naturally be inherited by the next member.

The essential problem is not that Agents lack more material. It is that the Project lacks a carrier
independent of any single member that can maintain shared knowledge and collaborative continuity
over the long term.

## 2. Definition of a “Project”

This system takes a Project—not one code repository, Agent Session, or temporary team—as its basic
unit.

A Project is:

> A boundary of coherent knowledge and decisions, maintained jointly by multiple Roles around a
> long-lived product, business domain, or software system.

A Project can include:

- multiple product surfaces and user touchpoints;
- multiple code repositories, services, modules, and their dependencies;
- source materials such as PRDs, designs, interfaces, tasks, and tests;
- runtime environments such as development, testing, staged rollout, and production;
- business and engineering Roles across product, design, development, testing, and operations;
- terminology, decisions, constraints, experience, and unresolved problems accumulated over time.

A Project persists over the long term. Requirements, releases, incidents, and focused initiatives
are work items inside the Project with beginnings and ends. A team is a composition of members
formed around particular work during one phase of the Project.

## 3. Core judgment: continuity belongs to the Project

The central shift in this project is to transfer ownership of continuity from an Agent to the
Project:

```text
Agent-centered model

Primary Agent / Leader
    ↓ creates a team, selects context, schedules work
Agent members
    ↓ return results
When the task or team ends, continuity depends on the original Agent retaining it
```

```text
Project-centered model

The Project persists over the long term
    ↕
Humans and Agents join, collaborate, leave, and are replaced
    ↕
Project knowledge, work state, decisions, and responsibility persist
```

Agents are members who can join, leave, and be replaced within the Project. No single Agent,
including a Leader, exclusively owns project context or is a necessary condition for the Project to
continue.

A new member should be able to restore the necessary understanding and current situation from the
Project before beginning work. When a member leaves, the critical knowledge, work responsibility,
and decision rationale already externalized still belong to the Project.

## 4. Project positioning

This project is positioned as:

> A shared knowledge and collaboration foundation attached to the Project, available to every
> authorized member, governed by Humans, and able to grow continuously with the Project.

More fully, it is:

> A Project-native collaboration space in which the Project is the persistent subject. Humans and
> Agents with independent lifecycles join as Project members with Roles, responsibilities, and
> permissions. They collaborate through Project-owned knowledge, work state, and governance rules.
> Members can change, but the Project’s knowledge, decisions, commitments, and work continuity do
> not disappear with any single member.

This positioning contains three related layers that must not be conflated:

```text
Shared knowledge layer       System kernel: maintains Project-specific, inheritable shared knowledge
Project collaboration space  Product form: carries members, work, responsibility, decisions, and handoff
Project-native A2A            Collaboration model: independent Agents work together as Project members
```

In this document, A2A emphasizes a collaboration relationship in which independent Agents share a
Project as their common anchor. It does not refer exclusively to any one communication protocol. A
communication protocol can carry interactions, but it cannot by itself provide Project
organizational memory, membership relationships, and governance.

## 5. System form

The system is neither a memory file attached to one repository nor an accessory tool of one Agent.
It takes the form of a long-lived project space independent of its current members.

Conceptually, the project space carries three kinds of institutional state:

### 5.1 Project knowledge

What the Project is, why it is this way, what it currently believes, what supports those beliefs,
where they apply, and where conflicts or unknowns remain.

Project knowledge is not a simple aggregation of source materials. It is the semantic increment that
affects Project action and cannot be derived cheaply and reliably from current code or materials
alone. It should have scope, evidence, and status, and it should be maintainable as the Project
changes.

### 5.2 Project collaboration

What the Project needs to accomplish now, who is taking responsibility for what, how work depends
on other work, what commitments members have made, what progress and blockers currently exist, and
how work is continued and handed off.

### 5.3 Project governance

Who may observe, suggest, act, and decide within which scope; how outputs are verified; how conflicts
are handled; when escalation is required; and how Humans retain ultimate governance over Project
goals, value boundaries, and high-risk matters.

Shared knowledge is a necessary foundation for collaboration, but making all Agents “know the same
things” is not enough to create collaboration. The project collaboration space must also maintain
active work state and public governance boundaries.

## 6. System goals

### 6.1 Establish continuity of Project knowledge

Project knowledge should not disappear when a Session ends, an Agent is replaced, a Leader leaves,
or a team is dissolved. Later members should continue from the knowledge the Project has already
formed instead of repeatedly excavating source materials.

### 6.2 Reduce the cost of joining a Project

Without requiring Humans to repeat the same onboarding, an authorized new member should be able to
obtain the minimum sufficient understanding needed for its current Role and task, and know how to
explore the Project further rather than loading all project materials at once.

### 6.3 Improve boundary correctness of Project action

The system should help members understand what a task applies to and what it does not apply to, how
product language maps to Project behavior and engineering objects, which constraints must be
respected, and where evidence is insufficient and clarification is required.

The primary benefit is reducing scope errors, invalid analogies, omissions, repeated mistakes, and
unnecessary action—not merely increasing search speed.

### 6.4 Support autonomous multi-Agent collaboration

Agents should not be limited to waiting for a Leader to dispatch work. Within authorization, the
Project can allow members to discover, propose, claim, negotiate, delegate, and continue work. It can
also use centralized scheduling when appropriate.

The system does not enforce one fixed organizational topology. It supports centralized leadership,
lateral collaboration, temporary groups, and autonomous claiming as forms that can coexist.

### 6.5 Eliminate single-point dependency for project context

A Leader can be responsible for direction, priorities, cross-domain tradeoffs, and necessary
arbitration without exclusively owning, remembering, and distributing the entire Project context.

Leadership is a replaceable Role granted by the Project, not the context owner of other members or
the parent process of the Project.

### 6.6 Let the Project grow continuously through work

Useful knowledge produced by requirements work, investigation, implementation, testing, review, and
handoff should return to the Project after appropriate governance so that future members can inherit
it.

The system should reduce knowledge that exists only in one person, one Agent, or one chat history,
while allowing Project knowledge to be corrected, challenged, superseded, and retired.

### 6.7 Preserve Human governance of the Project

Humans no longer need to serve as information relays and provide routine context between every
member, but they remain responsible for governing Project goals, value judgments, permission
boundaries, major risks, and irreversible decisions.

The goal is not to remove Human responsibility. It is to shift Humans from repeatedly supplying
context and micromanaging work toward defining boundaries, resolving critical disagreements, and
overseeing how the Project reaches decisions.

## 7. Members and Roles

### 7.1 Agent members

An Agent consumes Project knowledge and contributes both Project work and new knowledge. Agents can
come from different models, frameworks, or Providers and can have independent lifecycles.

The system does not require Agent processes to run forever. What must persist are member identity,
Role, permissions, commitments, work state, and historical contributions. A concrete Agent instance
can end, recover, or be replaced.

### 7.2 Human members and governors

Humans are first-class participants in the project space, not merely administrators outside the
system. Humans can contribute and correct Project knowledge, participate in work and decisions, and
hold ultimate governance authority in specified domains.

### 7.3 Leader

A Leader is a coordination and decision Role in the Project. A Leader can set priorities, address
responsibility gaps, organize work, resolve cross-domain conflicts, and make specified commitments
on behalf of the Project.

Leaders can be replaced, and multiple Leaders can exist by domain. A Leader’s authority comes from
responsibility granted by the Project, not from having created all other Agents.

## 8. Collaboration principles

### 8.1 The Project takes precedence over members

Members join and leave around the Project; the Project is not created and destroyed around an
Agent’s Session.

### 8.2 Project-centered does not mean ungoverned

The aim is for knowledge and collaboration not to depend on a single Agent, not for every member to
have equal authority over every matter. Responsibility and decision authority can be distributed by
Role, domain, risk, and issue.

### 8.3 A shared baseline does not mean forced consensus

Real Projects permit different perspectives, unverified judgments, conflicting information, and
unresolved disagreements. The system maintains a shared, traceable knowledge baseline rather than
compressing every member into one narrative.

### 8.4 The Project is an implicit third party in collaboration

Agents can communicate directly, but requests, commitments, conclusions, disagreements, decisions,
and handoffs that affect the Project cannot remain only in private member exchanges. They should
return to the Project in an appropriate form.

This is institutional ownership; it does not mean every message must be relayed through a central
system.

### 8.5 Minimum sufficient knowledge is better than complete injection

Every member shares the same Project knowledge foundation, but that does not mean every member
receives exactly the same complete content at every moment. The system should provide a Project view
sufficient for action according to Role, task, permissions, and current state.

### 8.6 Continuously externalize critical state

Project continuity cannot depend on one summary produced before a member exits. Important knowledge,
action intent, responsibility, progress, risk, and decisions should continuously become inheritable
Project state while work is underway.

## 9. Primary use cases

### 9.1 Joining a Project

A new Human or Agent member receives the Project map, current situation, Role boundaries, and
relevant work, forming the situational awareness needed to participate effectively.

### 9.2 Receiving or discovering work

A member can accept an assignment or discover, propose, claim, or negotiate work according to Role,
capability, and permissions, while understanding how it relates to Project goals, other work, and
historical decisions.

### 9.3 Collaboration across Roles and assets

Members in product, design, development, testing, operations, and other Roles can collaborate around
the same Project knowledge across product surfaces, repositories, Documents, and environments
without each rebuilding a disconnected context.

### 9.4 Decisions and verification

Members can identify what they may decide autonomously, which matters require other Roles, and which
questions must be submitted to Human governance. Important conclusions retain their rationale,
applicability boundaries, and responsibility attribution.

### 9.5 Handoff, exit, and replacement

When a member leaves or is replaced, unfinished responsibility, current progress, critical
judgments, risks, and unresolved questions remain in the Project so that other members can continue
the work.

## 10. Non-goals and boundaries

This project is not:

- an ordinary document-search, vector-retrieval, or knowledge-base product;
- a code assistant serving only one code repository;
- a new fact source that replaces original sources such as code, PRDs, and designs;
- a hidden super-Agent inside the system that understands and schedules everything;
- a fixed Agent Team that must be controlled by a single Leader;
- a centerless organization that completely eliminates Leaders, Role differences, or Human
  responsibility;
- a full-memory system that stores every chat, draft, and member-internal analysis;
- a mechanism guaranteeing that all Agents have exactly the same context or always reach consensus;
- a hosting system that requires Agent Sessions and Runtime instances to remain alive forever.

The system does not perform specialized work on behalf of an Agent and does not automatically become
the truth of the Project. It maintains shareable Project knowledge, collaboration state, and
governance boundaries, and helps members work together within those boundaries.

## 11. Target state

When this project reaches its goals, the following should be true:

- a new member can recover enough Project familiarity to work with little dependence on repeated
  Human explanation;
- Agent replacement, Session termination, and changes to team structure no longer cause wholesale
  loss of Project knowledge and work state;
- after a Leader is replaced, the Project can still restore its current situation and continue;
- multiple Agents can collaborate autonomously from shared Project knowledge while retaining clear
  Role, responsibility, and decision boundaries;
- critical judgments, commitments, and decisions in the Project can be traced, challenged, and
  corrected;
- unknown, conflicting, and stale information is exposed explicitly rather than packaged as a
  falsely coherent answer;
- every piece of work can make the Project easier for the next member to understand and continue,
  rather than creating more tacit knowledge that depends on personal explanation.

The long-term vision can be summarized in one sentence:

> Enable changing Human and Agent members to build sufficiently coherent and traceable working
> knowledge without making any single Agent a memory bottleneck for the Project; let them
> collaborate autonomously within explicit Role and decision boundaries; and preserve Project
> continuity throughout joining, working, handoff, and departure.

## 12. Project Space Constitution

Building on this positioning and these goals, a separate document further defines ownership of
project-space state, member rights and responsibilities, Agent autonomy, Human governance,
canonical writeback, and historical boundaries:

- [Project Space Constitution](project-space-constitution.md)

Project positioning and goals constrain the direction of system design. The Project Space
Constitution defines governance boundaries that Humans and Agents must not cross silently when
collaborating. Exact object relationships, permissions, and capability status remain governed by
the Core Model, domain specifications, and current implementation.

For the currently confirmed project-space structure, see the
[Carryforth Core Model](core-model.md).
