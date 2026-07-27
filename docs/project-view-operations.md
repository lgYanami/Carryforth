# Project View operations

This runbook covers the first server-side Project View release, routine
enable/disable operations, diagnostics, signer rotation, and rollback. Project
View uses the existing Relay, PostgreSQL, Redis, Nostr protocol, and
`buzz-admin`; it does not add a service or a Pod-local feature flag.

The safety invariant is:

> Roll every Relay Pod to the Project View-capable binary before migration 25,
> migrate before enabling any Community, and publish the Project View-capable
> `buzz` CLI only after the Relay can advertise the capability.

`communities.project_view_enabled` is the only runtime feature gate. Do not add
or set `BUZZ_PROJECT_VIEW_ENABLED`: a per-Pod value would make a rolling
deployment inconsistent.

## Prerequisites

- Pin one immutable `sha-*` Relay image that contains `buzz-relay`,
  `buzz-admin`, migration 25, and the Project View kind/read gates.
- Back up PostgreSQL and record the restore point. Database rollback is
  forward-only; the normal procedure never drops Project View data.
- Keep `BUZZ_RELAY_PRIVATE_KEY` stable and identical on every Pod. Its public
  key signs every Project View projection.
- Identify the normalized Community host used by NIP-11 and
  `buzz-admin --community`; examples below use `relay.example.com`.
- Prevent production clients from submitting kind `44300` until enablement is
  complete.

For Kubernetes examples, `$POD` means one Pod from the fully rolled
Project View-capable Deployment. It inherits the production database, Redis,
and signer environment:

```bash
POD="$(kubectl -n buzz get pod \
  -l app.kubernetes.io/name=buzz \
  -o jsonpath='{.items[0].metadata.name}')"
```

## Server-first rollout

### 1. Roll the binary with migration disabled

Set Helm `migrate.autoMigrate=false` (or
`BUZZ_AUTO_MIGRATE=false` in Compose), deploy the immutable image, and wait
until every Pod has the new image digest:

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

At this point old Buzz operations must remain healthy, Project View is
schema-not-ready/disabled, and NIP-11 must not advertise
`buzz-project-view-v1`.

### 2. Apply and verify migration 25

Run the admin binary from the same immutable image:

```bash
kubectl -n buzz exec "$POD" -- buzz-admin migrate
kubectl -n buzz exec "$POD" -- buzz-admin project-view status
```

Verify the ledger and additive compatibility:

```sql
SELECT version, success
FROM _sqlx_migrations
WHERE version = 25;

SELECT id, host, project_view_enabled
FROM communities
ORDER BY host;
```

Every Community must still be disabled. Also verify:

```bash
kubectl -n buzz exec "$POD" -- \
  curl -fsS http://127.0.0.1:9102/metrics |
  grep '^buzz_project_view_schema_ready '
```

The gauge must be `1`. Run the release image's Project View E2E/smoke in
staging before production:

```bash
just project-view-test-e2e
```

### 3. Enable one Community

The enable command takes the same advisory lock as mutations and validates the
schema, stable signer, and existing projection state before changing the
central database flag:

```bash
kubectl -n buzz exec "$POD" -- \
  buzz-admin project-view enable --community relay.example.com
kubectl -n buzz exec "$POD" -- \
  buzz-admin project-view status --community relay.example.com
```

NIP-11 for that host should now advertise `buzz-project-view-v1`; its `self`
must be the public key derived from the stable Relay secret. After
initialization, `project-view status` must show the same
`projection_pubkey`. Use a real member credential for the read smoke:

```bash
BUZZ_RELAY_URL=https://relay.example.com \
BUZZ_PRIVATE_KEY=nsec1... \
  buzz --format compact project-view get
```

Start with one staging/production canary Community. Enable additional
Communities only after the metrics below are stable. `--all` is supported, but
is intentionally not the first-rollout default:

```bash
buzz-admin project-view enable --all
```

## Monitoring

Project View exports only closed, low-cardinality labels:

```text
buzz_project_view_mutations_total{operation,result}
buzz_project_view_mutation_duration_seconds{operation}
buzz_project_view_conflicts_total{operation}
buzz_project_view_snapshot_duration_seconds
buzz_project_view_snapshot_retries_total{reason}
buzz_project_view_objects{type}
buzz_project_view_projection_dispatch_errors_total
buzz_project_view_schema_ready
```

Alert when schema readiness becomes `0` on an enabled deployment, internal or
unavailable mutation results persist, projection dispatch errors increase, or
snapshot `revision_changed` retries remain elevated. Conflicts can be normal
under concurrent writers, but a sustained spike usually means callers are not
refreshing the current project revision.

Mutation logs contain `community_host`, command/actor coordinates, bounded
operation/object type, object ID, expected/committed revisions, and result
code. They intentionally omit object bodies, patches, Resource locators, and
titles.

Useful database checks:

```sql
-- Canonical revision and meta projection must agree.
SELECT c.host,
       s.project_revision,
       (e.content::jsonb ->> 'project_revision')::bigint AS meta_revision,
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

-- Waiting advisory locks should normally be empty/short-lived.
SELECT pid, wait_event_type, wait_event, query_start
FROM pg_stat_activity
WHERE wait_event_type = 'Lock'
  AND wait_event = 'advisory';
```

## Disable and incident containment

Disable first when writes or projections are suspect:

```bash
kubectl -n buzz exec "$POD" -- \
  buzz-admin project-view disable --community relay.example.com
kubectl -n buzz exec "$POD" -- \
  buzz-admin project-view status --community relay.example.com
```

Disable takes the exclusive Community advisory lock, so after it commits no
older in-flight mutation can commit behind it. It removes the NIP-11 capability
and rejects new writes without deleting commands, receipts, canonical objects,
or projections. Readers remain protected by the Project View kind classifier
and membership gates.

## Signer rotation

Never change `BUZZ_RELAY_PRIVATE_KEY` while Project View is enabled. Disable
every affected Community, re-sign all current projections, update the
Kubernetes Secret, roll every Relay Pod to the new stable key, verify the new
NIP-11 `self`, and only then enable again:

```bash
buzz-admin project-view disable --community relay.example.com
buzz-admin project-view reproject \
  --community relay.example.com \
  --relay-key-file /run/secrets/new-relay-key \
  --expected-pubkey npub1...
buzz-admin project-view status --community relay.example.com
kubectl -n buzz rollout restart deployment/buzz
kubectl -n buzz rollout status deployment/buzz
BUZZ_RELAY_PRIVATE_KEY="$(cat /run/secrets/new-relay-key)" \
  buzz-admin project-view enable --community relay.example.com
```

The key file must be a regular file with no group/world permissions.
Reprojection increments `projection_generation` without changing
`project_revision`. The Secret update must precede the rollout restart; the
commands above intentionally omit the operator-specific Secret manager step.

## Rollback

The database only moves forward; there is no routine down migration.

- **After migration 25 but before the first enable/mutation:** keep
  `project_view_enabled=false`. A pre-feature Relay may be used only after its
  exact binary passes a v25-database smoke with `BUZZ_AUTO_MIGRATE=false`.
  Older SQLx migrators can reject an unknown version when auto-migration is
  left on. Prefer a rollback-compatible image containing the old business code
  plus migration 25.
- **After any Project View mutation has been accepted:** never deploy a binary
  that does not know kinds `44300`, `40903`, and `40904`. It lacks the strict
  membership read gate and could expose persisted protocol events through a
  global query. Disable the feature, let the advisory lock drain writers, and
  either forward-fix or deploy a rollback-compatible image that retains
  migration 25, the kind classifier, read gates, and centralized DB flag.
- Never delete Project View command events, receipts, object rows, or
  projections as part of application rollback. Restore a database backup only
  for verified database corruption, not normal release rollback.

After recovery, repeat status, SQL consistency, NIP-11 signer, member read
gate, and real CLI smoke checks before re-enabling.
