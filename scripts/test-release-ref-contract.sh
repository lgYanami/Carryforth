#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
verify="${repo_root}/scripts/verify-release-ref.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

git -C "$tmp" init -q
git -C "$tmp" config user.name test
git -C "$tmp" config user.email test@example.com
echo first >"$tmp/file"
git -C "$tmp" add file
git -C "$tmp" commit -qm first
git -C "$tmp" tag -m "desktop release" v1.2.3

(
  cd "$tmp"
  GITHUB_REF=refs/tags/v1.2.3 "$verify" v 1.2.3
)

if (
  cd "$tmp"
  GITHUB_REF=refs/heads/main "$verify" v 1.2.3
); then
  echo "branch-backed desktop release was accepted" >&2
  exit 1
fi

echo second >>"$tmp/file"
git -C "$tmp" commit -qam second
if (
  cd "$tmp"
  GITHUB_REF=refs/tags/v1.2.3 "$verify" v 1.2.3
); then
  echo "release accepted HEAD after the tag commit" >&2
  exit 1
fi

git -C "$tmp" tag -m "relay release" relay-v2.0.0
(
  cd "$tmp"
  GITHUB_REF=refs/tags/relay-v2.0.0 "$verify" relay-v 2.0.0
)

if grep -q 'inputs\.ref' \
  "$repo_root/.github/workflows/release.yml" \
  "$repo_root/.github/workflows/docker.yml"; then
  echo "publisher workflow still accepts a caller-selected source ref" >&2
  exit 1
fi

grep -q 'verify-release-ref\.sh' "$repo_root/.github/workflows/release.yml"
grep -q 'verify-release-ref\.sh' "$repo_root/.github/workflows/docker.yml"
grep -q 'check-open-source-release-surface\.sh --release-source' "$repo_root/.github/workflows/release.yml"
grep -q 'check-open-source-release-surface\.sh --release-source' "$repo_root/.github/workflows/docker.yml"
grep -q 'test-carryforth-local-deployment\.sh' "$repo_root/.github/workflows/release.yml"
grep -q 'test-carryforth-local-deployment\.sh' "$repo_root/.github/workflows/docker.yml"
grep -q 'scripts/test-ci-source-contracts\.sh' "$repo_root/.github/workflows/ci.yml"
grep -q 'test-release-ref-contract\.sh' "$repo_root/scripts/test-ci-source-contracts.sh"
"$repo_root/scripts/test-signed-canary-contract.sh"

release_workflows=(
  "$repo_root/.github/workflows/release.yml"
  "$repo_root/.github/workflows/docker.yml"
  "$repo_root/.github/workflows/signed-macos-canary.yml"
)

desktop_build_workflows=(
  "$repo_root/.github/workflows/release.yml"
  "$repo_root/.github/workflows/signed-macos-canary.yml"
  "$repo_root/.github/workflows/linux-canary.yml"
  "$repo_root/.github/workflows/windows-canary.yml"
)

for workflow in \
  "$repo_root/.github/workflows/linux-canary.yml" \
  "$repo_root/.github/workflows/signed-macos-canary.yml" \
  "$repo_root/.github/workflows/windows-canary.yml"; do
  grep -Fq 'check-release-asset-inventory.sh --release' "$workflow"
done

if grep -Eqi \
  'block/apple-codesign-action|github\.com/block/buzz|ghcr\.io/block|buzz-desktop-latest|Buzz\.app|BUZZ_UPDATER|SPROUT_UPDATER' \
  "${release_workflows[@]}"; then
  echo "public release workflows still depend on the legacy release surface" >&2
  exit 1
fi

if grep -Eq 'buzz-push-gateway|Dockerfile\.push-gateway' "$repo_root/.github/workflows/docker.yml"; then
  echo "the first public relay lane still publishes the unsupported push gateway" >&2
  exit 1
fi

if grep -Fq "if: github.event_name != 'pull_request'" "$repo_root/.github/workflows/docker.yml" ||
  grep -Fq 'push=${{ github.event_name != '\''pull_request'\'' }}' "$repo_root/.github/workflows/docker.yml"; then
  echo "the Relay workflow can still publish a floating main-branch image" >&2
  exit 1
fi

grep -Fq 'Relay package version matches the release tag' "$repo_root/.github/workflows/docker.yml"
grep -Fq "if: github.event_name == 'push' && github.ref_type == 'tag'" \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq 'workflow_dispatch is deliberately build/validation-only' \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq 'Verify provenance before canonical publication' \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq 'Publish write-once semver and movable aliases' \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq 'Full Relay semver tag ${full_tag} is write-once' \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq 'manifest unknown|name unknown|(^|[[:space:]:])not found' \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq -- '--source-digest "$GITHUB_SHA"' \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq -- '--source-ref "$GITHUB_REF"' \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq -- '--signer-workflow "github.com/${GITHUB_REPOSITORY}/.github/workflows/docker.yml"' \
  "$repo_root/.github/workflows/docker.yml"
grep -Fq 'movable_tags=(' "$repo_root/.github/workflows/docker.yml"
provenance_line=$(grep -Fn 'Verify provenance before canonical publication' \
  "$repo_root/.github/workflows/docker.yml" | cut -d: -f1)
anonymous_line=$(grep -Fn 'Verify the staged digest is anonymously readable' \
  "$repo_root/.github/workflows/docker.yml" | cut -d: -f1)
canonical_line=$(grep -Fn 'Publish write-once semver and movable aliases' \
  "$repo_root/.github/workflows/docker.yml" | cut -d: -f1)
if ! ((provenance_line < anonymous_line && anonymous_line < canonical_line)); then
  echo "Relay canonical aliases are published before provenance/public-read verification" >&2
  exit 1
fi
if grep -F 'push=${{' "$repo_root/.github/workflows/docker.yml" \
  | grep -Fq 'workflow_dispatch'; then
  echo "workflow_dispatch can still push Relay image data" >&2
  exit 1
fi
grep -Fq 'public release assets are immutable' "$repo_root/.github/workflows/release.yml"
if grep -Fq -- '--clobber' "$repo_root/.github/workflows/release.yml"; then
  echo "the public release workflow can overwrite an existing asset" >&2
  exit 1
fi

if grep -Fq 'cargo update --workspace' "${desktop_build_workflows[@]}"; then
  echo "a release or canary workflow can still re-resolve Cargo dependencies" >&2
  exit 1
fi
for workflow in "${desktop_build_workflows[@]}"; do
  grep -Fq -- '--locked' "$workflow"
done

grep -Fq 'cargo metadata --locked --no-deps --format-version 1' \
  "$repo_root/.github/workflows/release.yml"
grep -Fq 'Desktop release v${RELEASE_VERSION} does not match all committed Desktop versions' \
  "$repo_root/.github/workflows/release.yml"
grep -Fq 'desktop_version: ${{ steps.components.outputs.desktop_version }}' \
  "$repo_root/.github/workflows/release.yml"
grep -Fq 'desktop: $desktop_version' "$repo_root/.github/workflows/release.yml"
grep -Fq 'desktop_version: $desktop_version' "$repo_root/.github/workflows/release.yml"
if grep -Fq 'set-version-from-tag.mjs' "$repo_root/.github/workflows/release.yml"; then
  echo "the formal Desktop release still mutates committed version metadata" >&2
  exit 1
fi
grep -Fq 'carryforth-local-stack-${VERSION}.tar.gz' \
  "$repo_root/.github/workflows/release.yml"
grep -Fq 'carryforth-release-manifest-${VERSION}.json' \
  "$repo_root/.github/workflows/release.yml"
grep -Fq 'deploy/local/.env.example' "$repo_root/.github/workflows/release.yml"
grep -Fq 'ghcr.io/lgyanami/carryforth-relay' "$repo_root/.github/workflows/release.yml"
grep -Fq 'skopeo inspect' "$repo_root/.github/workflows/release.yml"
grep -Fq 'RELAY_IMAGE' "$repo_root/.github/workflows/release.yml"
grep -Fq 'Verify protected Relay tag and provenance' "$repo_root/.github/workflows/release.yml"
grep -Fq -- '--source-digest "$source_sha"' "$repo_root/.github/workflows/release.yml"
grep -Fq -- '--source-ref "$source_ref"' "$repo_root/.github/workflows/release.yml"
grep -Fq 'relay_source_sha' "$repo_root/.github/workflows/release.yml"
grep -Fq 'relay_provenance_sha256' "$repo_root/.github/workflows/release.yml"
grep -Fq 'carryforth-relay-provenance-${RELAY_VERSION}.json' \
  "$repo_root/.github/workflows/release.yml"
grep -Fq 'local_stack_runtime_dir="$local_stack_dir/deploy/local"' \
  "$repo_root/.github/workflows/release.yml"
grep -Fq '`deploy/local/RELAY_IMAGE`' "$repo_root/.github/workflows/release.yml"
grep -Fq 'cp LICENSE NOTICE UPSTREAM.md "$dist/cf/licenses/"' \
  "$repo_root/.github/workflows/release.yml"
grep -Fq 'COPY LICENSE NOTICE UPSTREAM.md /usr/share/licenses/carryforth/' \
  "$repo_root/Dockerfile"

grep -Fq 'community-unsigned' "$repo_root/.github/workflows/release.yml"
grep -Fq 'createUpdaterArtifacts: false' "$repo_root/desktop/scripts/build-release-config.mjs"
if grep -Eq 'plugins.*updater|UPDATER_|createUpdaterArtifacts: true' \
  "$repo_root/desktop/scripts/build-release-config.mjs"; then
  echo "release config generator can still inject the retired updater" >&2
  exit 1
fi

generated="$tmp/tauri.release.conf.json"
(
  cd "$repo_root/desktop"
  node scripts/build-release-config.mjs "$generated"
)
jq -e '.bundle.createUpdaterArtifacts == false' "$generated" >/dev/null
jq -e 'has("plugins") | not' "$generated" >/dev/null

desktop_package_version=$(jq -er '.version' "$repo_root/desktop/package.json")
desktop_tauri_version=$(jq -er '.version' "$repo_root/desktop/src-tauri/tauri.conf.json")
desktop_cargo_version=$(cargo metadata --locked --no-deps --format-version 1 \
  --manifest-path "$repo_root/desktop/src-tauri/Cargo.toml" \
  | jq -er '.packages[] | select(.name == "buzz-desktop") | .version')
desktop_lock_version=$(awk '
  BEGIN { RS = ""; FS = "\n"; found = 0; locked = "" }
  $0 ~ /(^|\n)name = "buzz-desktop"(\n|$)/ {
    found++
    for (line = 1; line <= NF; line++) {
      if ($line ~ /^version = "/) {
        locked = $line
        sub(/^version = "/, "", locked)
        sub(/"$/, "", locked)
      }
    }
  }
  END {
    if (found != 1 || locked == "") exit 1
    print locked
  }
' "$repo_root/desktop/src-tauri/Cargo.lock")
if [[ "$desktop_package_version" != "$desktop_tauri_version" ||
  "$desktop_package_version" != "$desktop_cargo_version" ||
  "$desktop_package_version" != "$desktop_lock_version" ]]; then
  echo "committed Desktop versions disagree: package=${desktop_package_version}, tauri=${desktop_tauri_version}, cargo=${desktop_cargo_version}, lock=${desktop_lock_version}" >&2
  exit 1
fi

if [[ -e "$repo_root/.github/workflows/auto-tag-on-release-pr-merge.yml" ]]; then
  echo "legacy private-App auto-tag workflow still exists; public releases require explicit protected tags" >&2
  exit 1
fi

echo "release ref contract passed"
