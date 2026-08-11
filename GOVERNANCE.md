# Carryforth Governance

Carryforth is an independently maintained open-source project. It is derived
from the Apache-2.0-licensed Buzz project but is not governed by Block, Inc. or
the upstream Buzz maintainers.

## Current maintainer model

The owner of the canonical
[`lgYanami/Carryforth`](https://github.com/lgYanami/Carryforth) repository is
the bootstrap maintainer and currently holds repository administration, merge,
and release authority. Listing a contributor in Git history, `CODEOWNERS`, or a
review request does not by itself grant release, security, or governance
authority.

This is an intentionally small initial model, not a claim that single-person
governance is the permanent structure. Maintainer additions or removals must be
recorded in a public pull request that updates this file and `CODEOWNERS`.

## Decision process

- Bugs and bounded implementation changes are proposed through GitHub issues
  or pull requests and resolved through review.
- Changes to identity, authorization, wire/storage contracts, migrations,
  local-only networking, data retention, or release policy require an explicit
  design record and Human maintainer approval.
- Destructive data actions, history rewrites, signing-key changes, and expansion
  to hosted services may not be inferred from an ordinary implementation task.
- When consensus is not reached, the bootstrap maintainer makes the final
  repository decision and records the reasoning publicly, except for
  confidential security details.
- A maintainer with a material conflict of interest should disclose it and
  delegate the decision where another maintainer is available.

## Contributor and maintainer responsibilities

Contributors are expected to follow [CONTRIBUTING.md](CONTRIBUTING.md), the
[Code of Conduct](CODE_OF_CONDUCT.md), and the repository's data-safety rules.
Maintainers additionally must:

- preserve Apache-2.0 and upstream attribution;
- keep public builds independent of private Block infrastructure;
- protect user identities, app data, databases, and Docker volumes;
- review migration and authorization changes proportionally to their risk;
- avoid presenting experimental/source-only components as supported releases;
- publish releases only from clean, reviewable, immutable source.

## Security and conduct

Security vulnerabilities must use the private process in
[SECURITY.md](SECURITY.md), never a public issue. General conduct reports also
must not include sensitive personal information in public issues.

The project still needs Human maintainer decisions for a durable private
conduct contact, security response targets, signing identities and custody,
and a succession/continuity policy. Those are release blockers and are not
silently inherited from the upstream project.

## Upstream relationship

Upstream changes may be reviewed and incorporated when they fit Carryforth's
local-only product and data-safety boundaries. Carryforth does not automatically
inherit upstream roadmap, support, governance, hosted services, trademarks, or
release artifacts. See [UPSTREAM.md](UPSTREAM.md) for the provenance policy.
