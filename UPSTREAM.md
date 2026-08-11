# Upstream and Provenance

Carryforth is derived from the
[`block/buzz`](https://github.com/block/buzz) source project, which was released
under the Apache License, Version 2.0. The repository's Git history, root
`LICENSE`, and `NOTICE` preserve that provenance.

## Independent project identity

The canonical Carryforth repository is
[`lgYanami/Carryforth`](https://github.com/lgYanami/Carryforth). Carryforth has
its own roadmap, governance, security process, release artifacts, and support
boundary. Upstream repositories, issue trackers, hosted services, private CI,
and signed artifacts are not Carryforth support or release channels.

Carryforth is not affiliated with, sponsored by, or endorsed by Block, Inc.
Upstream product and company names are used only where needed to describe
origin, history, or compatibility. The Apache-2.0 license does not grant
trademark rights.

## Compatibility names

Some inherited technical coordinates still use `buzz-*`, including Rust crate
and binary names, database tables, Nostr kinds, capabilities, environment
compatibility paths, and historical event values. They remain where changing
them would affect wire/storage compatibility or data continuity. Their presence
does not make Buzz the current product identity.

Do not mechanically rename those coordinates. Any change to a wire, storage,
bundle, keyring, or app-data identifier requires an explicit forward migration,
canonical readback, and existing-data acceptance plan.

## Incorporating upstream changes

Upstream changes may be incorporated after review against Carryforth's current
boundaries:

1. preserve the upstream commit and author provenance in Git;
2. retain applicable copyright and license notices;
3. reject dependencies on private Block infrastructure or credentials;
4. adapt hosted-account, updater, push, and remote-community assumptions to
   Carryforth's local-only model instead of silently restoring them;
5. preserve the complete forward migration chain and existing local data;
6. run Carryforth's tests and review authorization, networking, and release
   implications independently.

An upstream release or passing upstream CI does not by itself make a change a
supported Carryforth feature.

## Copyright and third-party material

Inherited source remains under its existing Apache-2.0 terms and notices.
Carryforth contributions are submitted under the same license unless a file
states otherwise. Third-party dependencies, fonts, images, sounds, provider
marks, and generated assets may have separate terms. Their inclusion in the
upstream repository is not, by itself, sufficient redistribution evidence for
a Carryforth binary release.

The complete dependency license/SBOM and packaged-asset provenance inventory is
still a release-readiness requirement. Material without confirmed
redistribution permission must be removed or replaced before it is packaged.
