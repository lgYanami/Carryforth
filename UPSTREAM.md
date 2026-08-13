# Upstream and Provenance

Carryforth is derived from the Apache-2.0-licensed
[`block/buzz`](https://github.com/block/buzz) source project. The exact source
anchor for Carryforth's reviewed import is
[`block/buzz@ab3af828714ab699dfc87644d234014987a4fe6b`](https://github.com/block/buzz/commit/ab3af828714ab699dfc87644d234014987a4fe6b).

The public Carryforth repository intentionally begins with a clean, squashed
source import. It does not reproduce the upstream commit ancestry. Provenance is
instead recorded by the exact public source anchor above, the retained upstream
`LICENSE` and applicable copyright notices, and Carryforth's independent
`NOTICE`. The complete pre-import history remains available in `block/buzz`.

## Independent project identity

The canonical Carryforth repository is
[`lgYanami/Carryforth`](https://github.com/lgYanami/Carryforth). Carryforth has
its own roadmap, governance, security process, and source-support boundary. Its
current public scope is source for local builds, evaluation, and study; it does
not publish stable binaries, installers, containers, or other packaged release
artifacts. Upstream repositories, issue trackers, hosted services, private CI,
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

1. record the exact upstream repository, commit, and applicable authorship in
   the Carryforth change; use a direct cherry-pick when appropriate, but do not
   rely on shared ancestry as the only provenance record;
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

The current source tree has a separate source-asset inventory. A complete
dependency license/SBOM and packaged-asset provenance inventory remains a
prerequisite only if Carryforth later publishes binaries, installers,
containers, or other artifacts. Material without confirmed redistribution
permission must be removed or replaced before it is packaged.
