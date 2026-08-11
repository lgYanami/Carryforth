#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
workflow="$repo_root/.github/workflows/signed-macos-canary.yml"

grep -Fq 'workflow_dispatch:' "$workflow"
# The literal GitHub expression is the contract we are checking.
# shellcheck disable=SC2016
grep -Fq 'SOURCE_REF: ${{ github.ref }}' "$workflow"
grep -Fq '"refs/heads/main"' "$workflow"
grep -Fq 'contents: read' "$workflow"
grep -Fq 'actions/upload-artifact@' "$workflow"
grep -Fq 'retention-days: 7' "$workflow"
grep -Fq 'build-release-config.mjs src-tauri/tauri.canary.conf.json' "$workflow"
grep -Fq -- '--no-sign' "$workflow"
grep -Fq 'community-unsigned.dmg' "$workflow"

if grep -Eqi 'contents: write|id-token: write|gh release|latest\.json|TAURI_SIGNING_PRIVATE_KEY|OSX_CODESIGN|CODESIGN_S3|verify-release-ref\.sh|refs/tags/|uses:.*codesign|codesign --|spctl --assess|block/buzz|block/apple|Buzz\.app|BUZZ_UPDATER|SPROUT_UPDATER|createUpdaterArtifacts[^f]*true' "$workflow"; then
  echo "macOS community canary gained signing, legacy updater, or publishing capability" >&2
  exit 1
fi

on_block=$(
  awk '
    /^on:$/ { in_on = 1; next }
    in_on && /^[^[:space:]#]/ { exit }
    in_on && NF && $0 !~ /^[[:space:]]*#/ {
      gsub(/[[:space:]]/, "")
      print
    }
  ' "$workflow"
)
if [[ "$on_block" != "workflow_dispatch:" ]]; then
  echo "macOS community canary workflow must have workflow_dispatch as its only trigger" >&2
  printf 'found on block:\n%s\n' "$on_block" >&2
  exit 1
fi

echo "unsigned macOS community canary contract passed"
