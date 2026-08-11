# Retired legacy Compose bundle

`deploy/compose` is retained only as a source-history tombstone for the former
Buzz/Block production deployment. It is **not** a supported or runnable
Carryforth deployment path.

The wrapper rejects every operational command, and `compose.yml` intentionally
defines no services. This prevents an old checkout, copied `.env`, or direct
`docker compose up` from pulling a floating legacy image or mutating an existing
database. These files do not remove or migrate any historical containers,
volumes, databases, or application data.

Use the local-only Carryforth stack instead:

```bash
cd deploy/local
./run.sh init --image "$(cat RELAY_IMAGE)"
./run.sh start
```

Follow [the local deployment guide](../local/README.md) for pinned Relay image
requirements, backup guidance, upgrades, and first-use initialization. Do not
copy settings from this retired directory into `deploy/local`.

`./run.sh help` prints this retirement notice. Commands such as `start`, `stop`,
`upgrade`, `config`, and member administration fail closed without invoking
Docker.
