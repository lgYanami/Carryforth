# Carryforth Mobile artwork

`carryforth.svg` is a byte-identical Mobile rendition of the canonical
`desktop/src-tauri/icons/carryforth-source.svg`. It is tracked inside the
Flutter package because Flutter asset bundles cannot depend on a path outside
the package root.

The Android launcher icons and iOS AppIcon catalog are generated from the same
canonical SVG with the lockfile-pinned Tauri CLI. The Mobile generator also
flattens iOS output onto `#20242B` and emits opaque RGB PNGs.

Regenerate or verify the full set from the repository root:

```sh
node mobile/scripts/generate-carryforth-icons.mjs --write
node mobile/scripts/generate-carryforth-icons.mjs --check
```

The generator reads no legacy Mobile icon and does not use the network.
