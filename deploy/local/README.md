# Carryforth local stack

This is the candidate local-only backend entrypoint under Carryforth release
qualification. It binds the Relay to `ws://localhost:3000` and starts only
Postgres, Redis, MinIO, and the Relay. It does not start Keycloak, Prometheus,
Push Gateway, a hosted Community service, or a remote fallback. The open
release blockers at the end of this document must be resolved before this entry
point is described as stable.

The first public stack targets Linux because the Relay container uses host
networking so the loopback-only first-owner claim remains genuinely loopback.
macOS and Windows must not be advertised until they have an equivalent owner
bootstrap and data migration path.

## Start

Use the `RELAY_IMAGE` digest coordinate included in the versioned Carryforth
local-stack archive:

```bash
cd deploy/local
./run.sh init --image "$(cat RELAY_IMAGE)"
./run.sh start
./run.sh status
```

`init` generates stable random secrets in `.env`, refuses floating tags and
never overwrites an existing file. Back up `.env`; it contains the stable Relay
identity and storage credentials. `run.sh` parses this generated file as data;
it never evaluates or sources it as shell code. Every command rejects missing,
duplicate, unknown, malformed, or overly-permissive configuration.
`./run.sh validate` performs this check without contacting Docker.

After the Relay is ready, start Carryforth Desktop. The first signed Desktop
identity claims the otherwise-empty local Community owner role through the
loopback-only owner bootstrap. This is local Nostr identity authorization, not
a Buzz/Builderlab account login.

## Lifecycle

```bash
./run.sh stop       # stop containers, retain every volume
./run.sh start      # restart the same state
./run.sh logs       # follow Relay logs
./run.sh upgrade --image ghcr.io/lgyanami/carryforth-relay:X.Y.Z@sha256:<release-digest>
./run.sh backup-hint
```

There is deliberately no reset or volume-deletion command. If an upgrade needs
a migration, the pinned Relay image applies its embedded, additive migration
chain before becoming ready. Release qualification must replay that same chain
against a scratch database and a read-only upgrade-baseline snapshot first.
The upgrade command rejects floating, non-canonical, same-version replacement,
and semantic-version downgrade coordinates and never removes
a named volume. A pull failure occurs before the new binary starts, so the
previous image pin can be restored safely. Once startup has begun, however, the
new Relay may already have applied a forward-only database migration. A failed
startup is therefore fail-closed: the Relay is stopped and the new pin is kept;
the script never launches the old binary against an unknown newer schema. Take
the printed backup before every upgrade and verify migration compatibility or
restore that backup before any explicit downgrade.

Exact semver tags are pinned coordinates, but registry tags can be moved by a
publisher; the release-qualified `:<semver>@sha256:<digest>` coordinate is
immutable and still carries the version needed for downgrade protection. The exact Postgres,
Redis, and MinIO tags in `compose.yml` are similarly reproducible coordinates,
not an immutability guarantee. Release evidence must record resolved digests.

`./run.sh config` validates the effective Compose model but prints only the
uninterpolated template. It never renders database, Redis, S3, Relay, or hook
secrets to the terminal.

## Network boundary

- Relay: `127.0.0.1:3000`
- Relay health and metrics: loopback-only high ports
- Postgres, Redis, and MinIO: loopback-only high ports
- no public TLS/domain mode
- no remote Community or fallback
- no Push delivery or Push Gateway

Model-provider network access is configured separately by the user for each
Agent harness and is not a Carryforth control-plane connection.

## Open release blockers

The local stack intentionally does not guess an Owner private key or forge an
Owner-signed capability event. Project View, Project Context, Meeting, and
other owner-governed capabilities still need a first-run Desktop bootstrap
that signs with the claimed local Owner identity. Until that guided bootstrap
is delivered and qualified, a fresh Relay being reachable is not proof that
every governed product capability is enabled.

Likewise, this stack does not migrate data from older development Compose
projects, volume names, or relay identities. Do not attach historical
development volumes to this stack. A documented, non-destructive migration and
rollback qualification remains a release blocker; existing local data must be
left untouched until that path exists.
