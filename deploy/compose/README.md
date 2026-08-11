# Retired legacy Compose bundle

`deploy/compose` is retained only as a source-history tombstone for the former
Buzz/Block production deployment. It is **not** a supported or runnable
Carryforth deployment path.

The wrapper rejects every operational command, and `compose.yml` intentionally
defines no services. This prevents an old checkout, copied `.env`, or direct
`docker compose up` from pulling a floating legacy image or mutating an existing
database. These files do not remove or migrate any historical containers,
volumes, databases, or application data.

For the current source-supported path, follow
[Build and run from source](../../README.md#build-and-run-from-source). A future
versioned local-stack bundle may use [the local deployment guide](../local/README.md)
with its generated `RELAY_IMAGE`; the source checkout does not provide that
release-qualified file. Do not copy settings from this retired directory into
`deploy/local`.

`./run.sh help` prints this retirement notice. Commands such as `start`, `stop`,
`upgrade`, `config`, and member administration fail closed without invoking
Docker.
