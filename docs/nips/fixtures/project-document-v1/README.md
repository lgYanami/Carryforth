# Project Document v1 Stage 0 fixtures

This directory is the shared, normative golden contract for NIP-PD and the
Project View v3 contracts needed by Document Guides and Context References.
Consumers must read these files directly; do not copy their JSON into a second
fixture tree.

Fixed identities:

| Value | Fixture |
|---|---|
| Project / Community | `3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77` |
| alternate Project | `872e6c7e-2cb1-4b7d-b59d-d6715d8217cd` |
| Document / Guide | `9c23f672-a397-42d1-b933-104ba2674f26` |
| Resource | `4f514f87-7b85-4c86-a50d-e01bc37e24c3` |
| Role | `db24ee5f-7d4c-48c3-826d-45f5a5db8428` |
| Assignment | `151f2347-7d24-41a0-ab0d-f272e84fcf88` |
| Runtime | `74ad5e95-903b-4488-ac19-d95a73fa62d4`, epoch `4` |
| Relay pubkey | `79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798` |
| member/reviewer pubkey | `c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5` |
| wrong signer pubkey | `f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9` |

The Nostr events use fixed test-only secret scalars `1`, `2`, and `3` for the
three public keys. They are public fixture material and must never be reused by
a real Relay or member. Canonical times are 2026-07-27 10:00, 10:05, and 10:10
UTC.

Contents:

- `commands/`: strict create, update, and delete JSON bodies;
- `events/`: signed command, active/tombstone head and revision, and
  empty/incremental/reset metadata events;
- `receipt-update.json`: stable business receipt with no projection pointer;
- `v3/`: Resource, every Context variant, RoleDefinitionV3, greenfield
  InitializeV3, and base Context-off RoleBriefV3;
- `migration/`: legacy input, lower-hex Human envelope, frozen postcard bytes,
  all domain-separated digests, and detached BIP-340 reviewer signature;
- `invalid/`: explicit-null malformed wire, extra tag, cross-Project, and
  wrong-signer cases that must fail closed.

`migration/golden.json` is byte-sensitive. Any postcard field-order or digest
change requires a new canonical schema and domain separator; updating the
expected hex in place is not a compatibility fix.
