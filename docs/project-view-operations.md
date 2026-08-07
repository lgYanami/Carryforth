# Project View v3 operations

This is the operational runbook for the current Project View runtime. Relay,
Desktop, ACP, and `buzz` use schema v3 only. Older schema payloads are accepted
only by explicitly named operator migration/recovery tools; they are never an
ordinary read, write, discovery, or client fallback path.

Project View uses the existing Relay, PostgreSQL, Redis, Nostr protocol, and
`buzz-admin`. `communities.project_view_enabled` is the central runtime gate.
Do not add `BUZZ_PROJECT_VIEW_ENABLED`: a per-Pod flag would make a rolling
deployment inconsistent.

The normal greenfield lifecycle is:

```text
schema-v3 Community (disabled, empty)
  -> NIP-11 advertises buzz-project-view-v3-bootstrap only
  -> prepare-v3
  -> owner-signed init-v3 (still disabled)
  -> bootstrap marker disappears; runtime v3 remains absent
  -> checked enable
  -> NIP-11 advertises buzz-project-view-v3
```

`prepare-v3`, `init-v3`, and `enable` are deliberately separate. Initialization
can be read back canonically before ordinary runtime advertisement, while only
the checked operator enable can publish the capability.

## Prerequisites

- Pin one immutable Relay image containing `buzz-relay`, `buzz-admin`, and the
  matching `buzz` CLI.
- Back up PostgreSQL and record the restore point. All normal schema changes are
  forward-only and additive; never delete Project View rows to roll back an
  application binary.
- Keep `BUZZ_RELAY_PRIVATE_KEY` stable and identical on every Relay Pod. Its
  public key signs every Project View projection.
- Identify the normalized Community host used by NIP-11 and
  `buzz-admin --community`; examples below use `relay.example.com`.
- Identify a current direct Human owner/admin public key. That identity signs
  `init-v3` and receives an initial admin Role Assignment.

For Kubernetes examples, `$POD` is one Pod from the fully rolled Deployment:

```bash
POD="$(kubectl -n buzz get pod \
  -l app.kubernetes.io/name=buzz \
  -o jsonpath='{.items[0].metadata.name}')"
```

## Server-first rollout

### 1. Roll the complete v3 binary set with automatic migration disabled

Set Helm `migrate.autoMigrate=false` (or `BUZZ_AUTO_MIGRATE=false` in Compose),
deploy the immutable image, and verify that every Pod has the same digest:

```bash
helm upgrade buzz oci://ghcr.io/block/buzz/charts/buzz \
  --reuse-values \
  --set image.tag=sha-abcdef0 \
  --set migrate.autoMigrate=false
kubectl -n buzz rollout status deployment/buzz
kubectl -n buzz get pods \
  -l app.kubernetes.io/name=buzz \
  -o jsonpath='{range .items[*]}{.metadata.name}{"  "}{.status.containerStatuses[0].imageID}{"\n"}{end}'
```

Do not deploy a new Relay with an older Desktop/ACP/CLI runtime and rely on a
compatibility fallback. All first-party surfaces move together and the static
v3 runtime gate enforces that boundary.

### 2. Apply and verify the current schema

Run migration from the same immutable image:

```bash
kubectl -n buzz exec "$POD" -- buzz-admin migrate
kubectl -n buzz exec "$POD" -- buzz-admin project-view status
```

Treat the status columns as a one-way lifecycle, not interchangeable feature
flags: `schema=3` selects the only ordinary runtime contract; `prepared=true`
means an unconsumed v3 preparation receipt exists; `initialized=true` means a
canonical state row exists; `strict-ready=true` means every canonical pointer
and signed projection passed the current v3 verifier; and `enabled=true` is
the final operator-controlled runtime switch. Enablement without strict
readiness is an incident, never a reason to fall back to an older schema.

Verify that the v3 greenfield default migration succeeded. It changes the
default for new Communities and installs fail-closed bootstrap validators; it
does not rewrite existing Project View data:

```sql
SELECT version, success
FROM _sqlx_migrations
WHERE version = 48;

SELECT column_default
FROM information_schema.columns
WHERE table_schema = 'public'
  AND table_name = 'communities'
  AND column_name = 'project_view_schema_version';
```

The default must be `3`. Also verify the release image without touching a
shared database:

```bash
./scripts/check-project-view-v3-runtime.sh
just project-view-test-e2e
```

The E2E creates and drops its own name-validated scratch database. It refuses
to run without its scratch-database sentinel.

### 3. Prepare one empty schema-v3 Community

A newly created Community is schema v3 but uninitialized and disabled. NIP-11
must advertise `buzz-project-view-v3-bootstrap`, not the ordinary
`buzz-project-view-v3` runtime capability. The bootstrap marker is discovery
only: it grants no read or write readiness. Create one idempotent provisioning
receipt:

```bash
BUZZ_PRIVATE_KEY=nsec1... \
  buzz-admin project-view prepare-v3 \
    --community relay.example.com \
    --idempotency-key pv3-prepare-2026-08-07 \
    --operator-pubkey npub1...
```

Record the returned `operation_id`. Preparation does not create Project state
and does not enable writes.

### 4. Initialize with the current Human owner

Create a closed `ProjectViewInitializeV3` JSON file. It must include the exact
preparation operation, profile, optional initial Goals, at least one active
admin Role, and an exact owner/admin-to-Role governance Assignment. Every UUID
is a client-generated UUID v4.

```json
{
  "schema_version": 3,
  "expected_project_revision": 0,
  "request": {
    "type": "initialize",
    "preparation_operation_id": "11111111-1111-4111-8111-111111111111",
    "profile": {
      "name": "Example",
      "positioning": "One canonical Project View",
      "purpose": "Coordinate the Community",
      "problem": "Project context is fragmented",
      "scope": "This Community"
    },
    "goals": [],
    "initial_roles": [{
      "role_id": "22222222-2222-4222-8222-222222222222",
      "name": "Community owner",
      "purpose": "Own initial Project governance",
      "responsibilities": ["Administer the Project"],
      "boundaries": ["Human governance only"],
      "level": "admin",
      "active": true,
      "context_references": []
    }],
    "initial_governance_assignments": [{
      "member_pubkey": "<owner lowercase hex public key>",
      "role_id": "22222222-2222-4222-8222-222222222222",
      "proposal_id": "33333333-3333-4333-8333-333333333333",
      "assignment_id": "44444444-4444-4444-8444-444444444444"
    }]
  }
}
```

Submit it through the current CLI:

```bash
BUZZ_RELAY_URL=https://relay.example.com \
BUZZ_PRIVATE_KEY=nsec1... \
  buzz --format compact project-view init-v3 --command initialize-v3.json
```

The CLI uses the bootstrap marker to discover this one closed initialization
path, then verifies the signed receipt and canonical schema-v3 projections
using the Relay `self` key. The Community intentionally remains disabled after
this step. The bootstrap marker must now be absent, while the ordinary runtime
capability remains absent until checked enable. Confirm `project_revision=1`, `projection_generation=1`,
`project_view_schema_version=3`, and `project_view_enabled=false`:

```bash
buzz-admin project-view status --community relay.example.com
```

### 5. Checked enable and smoke

Enable only after strict readiness verifies every current object, complete Role
continuity history, metadata, membership snapshot, signatures, exact tags, and
projection pointers:

```bash
buzz-admin project-view enable --community relay.example.com
buzz-admin project-view status --community relay.example.com
curl -fsS https://relay.example.com/info | jq '.supported_extensions, .self'
```

NIP-11 must advertise exactly `buzz-project-view-v3` for Project View, and
`self` must equal the configured projection signer. Use a real member identity
for read smoke:

```bash
BUZZ_RELAY_URL=https://relay.example.com \
BUZZ_PRIVATE_KEY=nsec1... \
  buzz --format compact project-view get

BUZZ_RELAY_URL=https://relay.example.com \
BUZZ_PRIVATE_KEY=nsec1... \
  buzz --format compact roles current
```

## Existing Communities and explicit migration

- A Community already at schema v3 uses only the ordinary lifecycle above.
- Never change an existing Community's schema column by hand.
- Older initialized Communities require the explicitly named operator
  migration/recovery workflow and frozen maintenance protocol. The isolated
  scratch-DB operator proof is
  `scripts/test-project-view-legacy-v2-to-v3-migration-canary.sh`.
- The legacy fixture is not part of ordinary runtime acceptance and must never
  be used against the main development or production database. It constructs
  canonical v2 state through internal operator APIs; it does not restore a v2
  CLI, Relay capability, or Desktop path.
- Project Documents used as migration input remain capability-disabled while
  the Community is on schema v2. Cutover checks the stable signer and complete
  canonical/current/history projection parity. Enable the Document capability
  only after v3 verify and resume complete.
- Any `buzz-admin project-document bootstrap` or `reproject` performed while
  the Community is on schema 2 must include `--for-v3-cutover`. The flag is an
  explicit migration acknowledgement and is rejected on schema 3; it never
  enables a schema-2 runtime capability.
- After migration, checked enable must pass the same complete v3 readiness
  scan. No first-party client retains an older-schema read/write fallback.

## Monitoring

Project View exports closed, low-cardinality metrics:

```text
buzz_project_view_mutations_total{operation,result}
buzz_project_view_mutation_duration_seconds{operation}
buzz_project_view_conflicts_total{operation}
buzz_project_view_snapshot_duration_seconds
buzz_project_view_snapshot_retries_total{reason}
buzz_project_view_objects{type}
buzz_project_view_projection_dispatch_errors_total
buzz_project_view_schema_ready
buzz_project_view_migration_required_communities
buzz_project_document_migration_required_communities
```

Alert immediately when either migration-required gauge is non-zero. It means an
active Community has an old Project View schema enabled: deployment readiness
is deliberately false and no first-party runtime will fall back to that major.
Disable the affected capability, freeze it, and run the explicit operator
cutover. All-disabled legacy Communities remain deployment-ready so that the
same current binary can perform that migration. Also alert when schema
readiness becomes `0` on an enabled Community, internal or unavailable mutation
results persist, projection dispatch errors increase, or conflicts remain
elevated. Conflicts are expected under occasional concurrent writes, but a
sustained spike usually means callers are not refreshing the current Project
revision.

Useful read-only checks:

```sql
-- Every enabled Community must be current schema v3.
SELECT host, project_view_schema_version, project_view_enabled
FROM communities
WHERE project_view_enabled
  AND project_view_schema_version <> 3;

-- Canonical revision and schema-v3 metadata must agree.
SELECT c.host,
       s.project_revision,
       (e.content::jsonb ->> 'project_revision')::bigint AS meta_revision,
       (e.content::jsonb ->> 'schema_version')::integer AS meta_schema,
       s.projection_generation,
       encode(s.projection_pubkey, 'hex') AS projection_pubkey
FROM project_view_state s
JOIN communities c ON c.id = s.community_id
LEFT JOIN events e
  ON e.community_id = s.community_id
 AND e.id = s.meta_projection_event_id
ORDER BY c.host;

-- Every canonical object must point at a persisted projection event.
SELECT c.host, o.object_id, o.object_type
FROM project_view_objects o
JOIN communities c ON c.id = o.community_id
LEFT JOIN events e
  ON e.community_id = o.community_id
 AND e.id = o.projection_event_id
WHERE e.id IS NULL;
```

## Disable and incident containment

Disable first when writes, signatures, or projections are suspect:

```bash
buzz-admin project-view disable --community relay.example.com
buzz-admin project-view status --community relay.example.com
```

Disable takes the exclusive Community advisory lock. After it commits, no
older in-flight mutation can commit behind it. It removes the NIP-11 capability
and rejects new writes without deleting commands, receipts, canonical objects,
continuity history, or projections.

## Signer rotation and v3 reprojection

Never change `BUZZ_RELAY_PRIVATE_KEY` while Project View is enabled. Signer
rotation uses the durable v3 maintenance state machine, not the old ordinary
reproject command:

1. `maintenance begin` disables writes and freezes the exact Assignment/Runtime
   baseline.
2. Wait for `maintenance ack-probe`, then `maintenance freeze`.
3. Run `maintenance reproject` with the new protected key file and expected
   public key.
4. Run `maintenance verify`, update the Secret, roll all Pods, and confirm the
   new NIP-11 `self`.
5. Run `maintenance resume`; checked readiness re-enables only a valid v3
   Community.

The key file must be a regular file with no group/world permissions.
Reprojection increments `projection_generation` without changing
`project_revision`. Keep the Community frozen if verification or Redis fan-out
fails; repair forward and resume the same exact epoch.

## Rollback

The database only moves forward; there is no routine down migration.

- Before initialization, leave the Community disabled and forward-fix the
  binary or provisioning input.
- **After any Project View mutation has been accepted**, never deploy a binary
  that does not implement the current Project View kinds, membership gates,
  schema-v3 projection parsers, and complete Role-history readers.
- There is no supported old-runtime application rollback after initialization.
  Disable new writes, keep the current schema-v3 binary serving verified reads,
  and forward-fix the current schema-v3 runtime as one Relay/CLI/Desktop/ACP
  release set. Do not restore an older ordinary Project View CLI or Relay as a
  compatibility path.
- Never delete Project View command events, receipts, objects, Role continuity
  history, Documents, Resources, Context references, or projections as part of
  rollback.
- Restore a database backup only for verified database corruption, never for a
  normal application rollback or failed client release.

After recovery, repeat status, strict readiness, signer, NIP-11, member read,
Role-history, and real CLI smoke checks before enabling.

The pre-feature database smoke in CI has a narrower purpose: it proves that
additive migrations do not prevent a Relay with no Project View feature from
starting while every Project View capability remains disabled and no Project
View state exists. It is not a post-mutation rollback qualification. Historical
schema data is covered only by the explicit, scratch-database
`test-project-view-legacy-v2-to-v3-migration-canary.sh` operator migration
canary.
