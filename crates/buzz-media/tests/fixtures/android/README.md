# Android Bitmap media fixtures

These fixtures contain only synthetic pixels. They were introduced in commit
`ee21da90bd6b1da6bfaaf22ba00749398aaa9640` and produced with Android 16
(API 36) `Bitmap.compress`, not copied from an external image or encoded by a
generic desktop tool. They are distributed under the repository's Apache-2.0
license.

The tracked [`GenerateFixtures.java`](GenerateFixtures.java) file is the
authoritative pixel and encoder definition. It creates:

- a 3 x 2 ARGB grid made from primary colors and three alpha values;
- a 3 x 2 Display-P3 RGBA_F16 red grid;
- PNG and quality-100 JPEG output for each grid.

Exact canonical and sanitized hashes are recorded in
[`../fixture-manifest.json`](../fixture-manifest.json).

## Regeneration

1. Compile and run `GenerateFixtures.java` on an Android 16 / API 36 emulator.
   It writes the four source files to `/data/local/tmp/`.
2. Pull those files into this directory.
3. Run the tracked sanitizer path for both source color spaces and formats.
   Save the four outputs under `sanitized/` with the `-sanitized` suffix.
4. Update only the matching Android hashes in `../fixture-manifest.json`, then
   run:

   ```sh
   node crates/buzz-media/tests/fixtures/check-fixtures.mjs
   cargo test -p buzz-media android_
   ```

The checker fails if any duplicate differs. `bitmap-srgb.png` is intentionally
identical to its sanitized form because its original chunks already satisfy the
sanitizer allowlist; that single equal-hash relationship is explicit in the
manifest rather than treated as an accidental duplicate.

Regenerate encoded and sanitized outputs together whenever Android's encoder or
the sanitizer changes.
