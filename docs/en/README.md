# Carryforth English Documentation

This is the entry point for Carryforth's English documentation. The root
[README.md](../../README.md) presents the product and the shortest getting-started path. Dedicated
documents describe the model, runtime workflow, system boundaries, and current maturity.

## Suggested reading order

1. [Core model](core-model.md)

   Understand why continuity belongs to the project, and how Project View, Role Continuity,
   Documents, Project Context, Meetings, and Members relate.

2. [Core design: Role Continuity](core-design/role-continuity.md)

   Understand how the Project separates long-lived Roles, Assignment tenures, Work
   Responsibility, Commitments, Checkpoints, Handoffs, and the derived Role Brief so that
   responsibility survives agent and runtime replacement.

3. [Core design: Coordinates before context](core-design/coordinate-and-context.md)

   Understand why stable coordinates, undirected Edges/Hyperedges, and versioned Documents have
   distinct responsibilities, and how agents discover and maintain related context from their
   current work coordinates.

4. [Core design: Agent-directed context-aware Project Context retrieval](core-design/context-aware-semantic-graph-retrieval.md)

   Understand how Agents use their current Role and relevant work environment to progressively
   select different yet related, traceable paths through one Project-owned Context Graph.

5. [Core design: Meeting](core-design/meeting.md)

   Understand how humans and agents aggregate context from different Roles, Work, and project
   experience, form an actionable shared conclusion, and explicitly write outcomes back to the
   Project.

6. [System overview](system-overview.md)

   Understand how Desktop, Relay, managed agents, `cf`, and local dependencies work together,
   including identity, permission, data-isolation, and network boundaries.

7. [Local development](local-development.md)

   Start from an environment that has never run Carryforth and work through prerequisite checks,
   Provider configuration, build, startup, rebuild, and shutdown.

8. [Current status](current-status.md)

   Distinguish implemented capabilities from explicitly gated, still-qualifying, and uncommitted
   surfaces.

## Product and governance documents

- [Project positioning and goals](project-positioning.md)
- [Project Space Constitution](project-space-constitution.md)
- [Project View definition](../stage/project-view/project-view.md)
- [Role Continuity](../stage/role/role-continuity.md)
- [Project Document](../stage/document/document.md)
- [Project Context](../stage/project-context/project-context.md)

The `docs/stage/` tree also contains historical phase designs, implementation plans, bug fixes,
and qualification records. Those documents capture engineering facts at specific points in time;
not every planned item should be read as a currently enabled product capability.

## Development and operations references

- [`cf` CLI function reference](cli-reference.md)
- [System architecture](../../ARCHITECTURE.md)
- [Contributing](../../CONTRIBUTING.md)
- [Testing guide](../../TESTING.md)
- [Security model and vulnerability reporting](../../SECURITY.md)
- [Project governance](../../GOVERNANCE.md)
- [Semantic pgvector operations](../semantic-pgvector-operations.md)
- [Upstream provenance and compatibility](../../UPSTREAM.md)

## Documentation boundary

- English topic documents explain the current product and source workflow; they do not replace
  protocol, migration, or operations contracts.
- If a topic document differs from code, migrations, or an active operations document, verify the
  current implementation first and correct the documentation.
- Names such as `buzz-*` and `BUZZ_*` may be compatibility contracts and must not be mechanically
  renamed merely to normalize prose.
- Never add keys, real private addresses, user content, or internal infrastructure details to
  public documentation.
