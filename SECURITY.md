# Security Policy

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Report suspected Carryforth vulnerabilities through GitHub's
[private vulnerability reporting form](https://github.com/lgYanami/Carryforth/security/advisories/new).
Include as much detail as possible:

- A description of the vulnerability and its potential impact
- Steps to reproduce or a proof-of-concept (if available)
- The affected version(s) or commit range
- Any suggested mitigations you've identified

Private vulnerability reporting must be enabled and the form above verified
before Carryforth source history is published in the public repository. GitHub
offers this setting only on public repositories, so a new repository must be
made public while empty, have reporting enabled and verified, and only then
receive the public source history. If the form is unexpectedly unavailable,
open a public issue titled `Private vulnerability reporting is unavailable`,
but include no vulnerability details, reproduction steps, logs, or attachments.
That issue is only a request for the maintainer to restore the private channel.

Carryforth has no security email address and makes no response-time service-level
commitment. Reports are handled on a best-effort basis; this policy does not
inherit an upstream project's contact details or response targets.

We ask that you:

- Give us reasonable time to address the issue before any public disclosure
- Avoid accessing or modifying data that does not belong to you
- Not perform denial-of-service attacks or disrupt production systems

We will credit reporters in a published advisory when appropriate unless they
prefer to remain anonymous.

---

## Supported Versions

| Source | Security maintenance status |
|--------|-----------------------------|
| Current `main` | Best-effort fixes during active development; no SLA |
| Carryforth stable versions | None published |
| Upstream Buzz releases | Not maintained as Carryforth versions |

Carryforth currently provides source for local building and evaluation. It does
not provide production support or maintain long-term support branches. A future
binary release policy must identify exact supported versions and response
expectations rather than inheriting an upstream promise.

---

## Security Design Principles

### Authentication — NIP-42

Relay WebSocket sessions use
[NIP-42](https://github.com/nostr-protocol/nips/blob/master/42.md)
challenge/response before protected event or query operations. The relay sends
a random challenge; the client signs a `kind:22242` event containing the
challenge and the relay URL, proving possession of the private key.

Protected operations on the generic HTTP event/query/count bridge authenticate
via
[NIP-98](https://github.com/nostr-protocol/nips/blob/master/98.md) HTTP Auth —
the client signs a `kind:27235` event containing the request URL and method.
The relay verifies the Schnorr signature and extracts the pubkey. Public health
and metadata routes, and routes with an explicit endpoint-specific credential
such as a webhook secret, do not use this generic NIP-98 rule.

### Authorization — Community, Channel, and Governance Boundaries

Community membership and member level establish the principal's base authority.
Channel membership scopes channel visibility and messaging. Project View,
Documents, Project Context, managed Agents, and Meetings add their own
well-defined governance and lifecycle checks. Authentication alone never grants
access to community data.

Private channels are invisible to non-members: they do not appear in channel
listings, and subscription filters for private channel events return nothing
unless the subscriber is a member.

### Optional Tamper-Evident Audit Log

When audit logging is enabled, covered operations are written to `buzz-audit`.
Each entry is chained to the previous one with SHA-256. Nostr `KIND_AUTH` and
ephemeral events are not recorded as event-created audit entries, and disabling
audit logging also disables these writes.

The keyless chain can reveal accidental corruption or edits that were not
accompanied by a chain rewrite. It is not tamper-resistant: an actor with
sufficient database write access can modify entries and recompute the chain.
This mechanism is not, by itself, a compliance certification, independent
attestation, or substitute for access controls and external backups.

### Desktop Secret Storage — OS Keyring

Default Carryforth Desktop builds prefer the operating-system keyring for human
and managed-Agent nsec private keys: macOS Keychain, Windows Credential Manager,
or the Linux Secret Service. Migration from a legacy plaintext key writes and
reads the key back from the keyring before deleting the legacy file.

If a keyring backend is unavailable, persistence paths retain or fall back to
owner-only `0o600` files rather than discarding the only known key. A previously
keyring-backed identity with no file fallback enters recovery mode instead of
silently persisting a new identity. The `BUZZ_PRIVATE_KEY` environment variable,
when set, takes precedence over persisted human identity stores; managed Agent
runtime keys are injected separately by the Desktop supervisor.

### Input Validation

- All UUIDs (channel IDs, workflow IDs) are validated at API boundaries before
  use in database queries.
- Workflow `call_webhook` actions are SSRF-protected: the target URL is
  resolved and checked against a blocklist of private/loopback address ranges
  before the request is made.
- Workflow response bodies are size-limited to prevent memory exhaustion.
- `evalexpr` condition expressions are length-limited and their awaited
  evaluation is timeout-bounded; they are not a general code-execution sandbox.
- Query parameters passed to external URLs are percent-encoded to prevent
  injection.

### Transport Security

Any deployment exposed beyond a trusted local machine should terminate TLS at
the relay or at a reverse proxy in front of it. The relay does not itself
enforce TLS. The repository's local-development defaults are not a production
hardening profile.

### Dependency Management

Most core service and domain crates enforce `#![deny(unsafe_code)]`. A small
number of platform-facing components, including the Desktop shell and Git
signing integration, contain scoped `unsafe` blocks for operating-system APIs;
changes to those blocks require focused review.

CI runs `cargo-deny check`, including advisory and license policy from
`deny.toml`. Explicitly accepted advisory exceptions are documented in that
file and must be reviewed when dependencies change. Passing this gate is not a
claim that the source or a future binary has received an independent security
audit.

---

## Disclosure Policy

We follow [coordinated disclosure](https://en.wikipedia.org/wiki/Coordinated_vulnerability_disclosure).
When appropriate, after a mitigation is available, the maintainer will publish
a GitHub security advisory describing the vulnerability, its impact, and the
mitigation. No publication or response deadline is promised. Reporters will be
credited unless they request anonymity.
