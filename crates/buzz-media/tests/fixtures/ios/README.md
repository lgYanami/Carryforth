# UIKit media fixtures

These fixtures contain only synthetic pixels. They were introduced in commit
`37f15b20019169363b697aee41c99573b7bc3f24` and produced by UIKit on an iOS
simulator, not copied from an external image. They are distributed under the
repository's Apache-2.0 license.

The input is now tracked at
[`../source/pixel-grid-2x2.png`](../source/pixel-grid-2x2.png): two red pixels
followed by two green pixels. Its standard-library-only Node generator defines
every source pixel and can verify or reproduce the input offline:

```sh
node crates/buzz-media/tests/fixtures/source/generate-pixel-source.mjs --check
node crates/buzz-media/tests/fixtures/source/generate-pixel-source.mjs --write
```

`GenerateFixtures.swift` is the authoritative UIKit re-encoding program. Exact
encoded/sanitized hashes and the Mobile copy relationships are recorded in
[`../fixture-manifest.json`](../fixture-manifest.json).

## Regeneration

1. Record the encoder environment alongside the change:

   ```sh
   xcodebuild -version
   xcrun --sdk iphonesimulator --show-sdk-version
   xcrun simctl list devices booted
   ```

2. Compile the tracked generator for the arm64 iOS 16.0 simulator target and
   run it against the tracked source:

   ```sh
   SDK_PATH="$(xcrun --sdk iphonesimulator --show-sdk-path)"
   xcrun --sdk iphonesimulator swiftc \
     -sdk "$SDK_PATH" \
     -target arm64-apple-ios16.0-simulator \
     crates/buzz-media/tests/fixtures/ios/GenerateFixtures.swift \
     -o /tmp/carryforth-generate-ios-fixtures
   xcrun simctl spawn booted /tmp/carryforth-generate-ios-fixtures \
     "$PWD/crates/buzz-media/tests/fixtures/source/pixel-grid-2x2.png" \
     /tmp/uikit-encoded.png /tmp/uikit-encoded.jpg
   ```

3. Copy the encoded files into this directory and into
   `mobile/ios/RunnerTests/Fixtures/`. Run the production
   `MediaSanitizer.scrubPng` and `scrubJpeg` paths to create both sanitized
   outputs; do not substitute a different image encoder.
4. Update only the matching iOS hashes in `../fixture-manifest.json`, then run:

   ```sh
   node crates/buzz-media/tests/fixtures/check-fixtures.mjs
   cargo test -p buzz-media test_ios_uikit
   ```

The checker verifies that both UIKit PNGs decode to the exact tracked synthetic
scanlines and that the Runner copies remain byte-identical. Regenerate encoded
and sanitized pairs together whenever UIKit or `MediaSanitizer` changes.
