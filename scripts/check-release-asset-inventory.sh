#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

manifest="docs/release/packaged-assets.json"
release_mode=false
release_obligation_evidence_schema="carryforth.release-obligation-evidence/v1"

usage() {
  cat <<'EOF'
Usage: scripts/check-release-asset-inventory.sh [--release]

Without --release, validate the explicit packaged-asset inventory and report
remaining blockers. With --release, fail if any inventory item or release
obligation is still blocked. A cleared release obligation is accepted only
with tracked, tag-bound evidence whose schema and SHA-256 validate.
EOF
}

case "${1:-}" in
  "") ;;
  --release) release_mode=true ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

for command in git jq sha256sum cargo; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "release asset inventory: missing required command: ${command}" >&2
    exit 2
  fi
done

if [[ ! -f "${manifest}" ]]; then
  echo "release asset inventory: missing ${manifest}" >&2
  exit 1
fi

jq -e --arg obligation_evidence_schema "${release_obligation_evidence_schema}" '
  .schema_version == 2
  and .release_obligation_evidence_schema == $obligation_evidence_schema
  and (.filesystem_assets | type == "array" and length > 0)
  and (.font_dependencies | type == "array")
  and (.packaged_programs | type == "array" and length > 0)
  and (.release_obligations | type == "array")
  and all(
    .filesystem_assets[];
    (.id | type == "string" and length > 0)
    and (.path_pattern | type == "string" and length > 0)
    and (.tracked_file_count | type == "number" and . > 0)
    and (.tree_sha256 | test("^[0-9a-f]{64}$"))
    and (.license | type == "string" and length > 0)
    and (.release_status == "cleared" or .release_status == "blocked")
    and (.provenance | type == "string" and length > 0)
  )
  and all(
    .font_dependencies[];
    (.id | type == "string" and length > 0)
    and (.coordinate | type == "string" and length > 0)
    and (.license | type == "string" and length > 0)
    and (.release_status == "cleared" or .release_status == "blocked")
  )
  and all(
    .packaged_programs[];
    (.id | type == "string" and length > 0)
    and (.artifact_name | type == "string" and length > 0)
    and (.source_package | type == "string" and length > 0)
    and (.license | type == "string" and length > 0)
    and (.release_status == "cleared" or .release_status == "blocked")
  )
  and all(
    .release_obligations[];
    (.id | type == "string" and length > 0)
    and (.release_status == "cleared" or .release_status == "blocked")
    and (.description | type == "string" and length > 0)
    and (
      if .release_status == "cleared" then
        (.evidence | type == "object")
        and (.evidence.schema == $obligation_evidence_schema)
        and (.evidence.path | type == "string" and test("^docs/release/evidence/[^/]+/[^/]+[.]json$"))
        and (.evidence.sha256 | type == "string" and test("^[0-9a-f]{64}$"))
        and (.evidence.release_tag | type == "string" and test("^v(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$"))
      else
        ((has("evidence") | not) or .evidence == null)
      end
    )
  )
  and (
    [
      .filesystem_assets[].id,
      .font_dependencies[].id,
      .packaged_programs[].id,
      .release_obligations[].id
    ] as $ids
    | ($ids | length) == ($ids | unique | length)
  )
' "${manifest}" >/dev/null || {
  echo "release asset inventory: ${manifest} does not satisfy schema v2" >&2
  exit 1
}

# These obligations are part of the first public release contract, not
# advisory prose. Keeping the list here prevents a manifest edit from silently
# removing a gate instead of supplying evidence that closes it.
required_release_obligations=(
  "binary-dependency-license-and-sbom-evidence"
  "relay-runtime-container-provenance"
  "bundle-identity-and-data-migration"
  "owner-signed-project-capability-bootstrap"
  "existing-data-upgrade-migration-readback"
  "clean-room-published-artifact-e2e"
  "human-private-reporting-and-release-governance"
)

for obligation_id in "${required_release_obligations[@]}"; do
  obligation_count="$(
    jq -r --arg obligation_id "${obligation_id}" \
      '[.release_obligations[] | select(.id == $obligation_id)] | length' \
      "${manifest}"
  )"
  if [[ "${obligation_count}" -ne 1 ]]; then
    echo "release asset inventory: required obligation ${obligation_id} occurs ${obligation_count} time(s), expected exactly one" >&2
    exit 1
  fi
done

# This is an intentionally closed list. It covers media trees known to feed
# the first Desktop package; it does not guess that unrelated fixtures or docs
# are release assets.
asset_pathspecs=(
  "desktop/public/carryforth.svg"
  "desktop/public/landing/carryforth-wordmark.svg"
  "desktop/src-tauri/icons/**"
  "desktop/src/shared/ui/assets/card-texture.png"
  "desktop/public/pow/**"
  "desktop/public/sounds/**"
)

mapfile -t tracked_assets < <(
  for pathspec in "${asset_pathspecs[@]}"; do
    while IFS= read -r asset; do
      if [[ -f "${asset}" ]]; then
        printf '%s\n' "${asset}"
      fi
    done < <(git ls-files --cached --others --exclude-standard -- "${pathspec}")
  done | sort -u
)
mapfile -t inventory_patterns < <(
  jq -r '.filesystem_assets[].path_pattern' "${manifest}"
)

if [[ ${#tracked_assets[@]} -eq 0 ]]; then
  echo "release asset inventory: explicit asset roots resolved no tracked files" >&2
  exit 1
fi

for asset in "${tracked_assets[@]}"; do
  match_count=0
  for pattern in "${inventory_patterns[@]}"; do
    if [[ "${asset}" == ${pattern} ]]; then
      ((match_count += 1))
    fi
  done
  if [[ ${match_count} -ne 1 ]]; then
    echo "release asset inventory: ${asset} matched ${match_count} inventory entries (expected exactly one)" >&2
    exit 1
  fi
done

for pattern in "${inventory_patterns[@]}"; do
  mapfile -t matches < <(
    for asset in "${tracked_assets[@]}"; do
      if [[ "${asset}" == ${pattern} ]]; then
        printf '%s\n' "${asset}"
      fi
    done
  )

  expected_count="$(
    jq -r --arg pattern "${pattern}" \
      '.filesystem_assets[] | select(.path_pattern == $pattern) | .tracked_file_count' \
      "${manifest}"
  )"
  expected_hash="$(
    jq -r --arg pattern "${pattern}" \
      '.filesystem_assets[] | select(.path_pattern == $pattern) | .tree_sha256' \
      "${manifest}"
  )"

  if [[ ${#matches[@]} -ne ${expected_count} ]]; then
    echo "release asset inventory: ${pattern} contains ${#matches[@]} tracked files; inventory records ${expected_count}" >&2
    exit 1
  fi

  actual_hash="$({
    for asset in "${matches[@]}"; do
      sha256sum "${asset}"
    done
  } | sha256sum | awk '{print $1}')"
  if [[ "${actual_hash}" != "${expected_hash}" ]]; then
    echo "release asset inventory: ${pattern} content changed" >&2
    echo "  expected tree_sha256: ${expected_hash}" >&2
    echo "  actual tree_sha256:   ${actual_hash}" >&2
    exit 1
  fi
done

# Keep the legal inventory tied to the actual programs Tauri packages.
mapfile -t tauri_sidecars < <(
  jq -r '.bundle.externalBin[]' desktop/src-tauri/tauri.conf.json | sort
)
mapfile -t inventory_sidecars < <(
  jq -r \
    '.packaged_programs[] | select(.destinations | index("Desktop sidecar")) | .artifact_name' \
    "${manifest}" | sort
)

if [[ "$(printf '%s\n' "${tauri_sidecars[@]}")" != "$(printf '%s\n' "${inventory_sidecars[@]}")" ]]; then
  echo "release asset inventory: Tauri externalBin and packaged_programs differ" >&2
  diff -u \
    <(printf '%s\n' "${tauri_sidecars[@]}") \
    <(printf '%s\n' "${inventory_sidecars[@]}") >&2 || true
  exit 1
fi

metadata="$(cargo metadata --locked --no-deps --format-version 1)"
while IFS=$'\t' read -r package expected_license; do
  actual_license="$(
    jq -r --arg package "${package}" \
      '.packages[] | select(.name == $package) | .license // empty' \
      <<<"${metadata}"
  )"
  if [[ -z "${actual_license}" ]]; then
    echo "release asset inventory: Cargo package not found or has no license metadata: ${package}" >&2
    exit 1
  fi
  if [[ "${actual_license}" != "${expected_license}" ]]; then
    echo "release asset inventory: ${package} license is ${actual_license}; inventory records ${expected_license}" >&2
    exit 1
  fi
done < <(
  jq -r '
    .packaged_programs[]
    | select(.source_package != "desktop/src-tauri")
    | [.source_package, .license]
    | @tsv
  ' "${manifest}"
)

desktop_license="$(sed -n 's/^license = "\([^"]*\)"/\1/p' desktop/src-tauri/Cargo.toml | head -1)"
if [[ "${desktop_license}" != "Apache-2.0" ]]; then
  echo "release asset inventory: desktop/src-tauri license is ${desktop_license:-missing}, expected Apache-2.0" >&2
  exit 1
fi

inter_spec="$(jq -r '.dependencies["@fontsource-variable/inter"] // empty' desktop/package.json)"
if [[ "${inter_spec}" != "^5.2.8" ]]; then
  echo "release asset inventory: Inter dependency changed from inventoried ^5.2.8 to ${inter_spec:-missing}" >&2
  exit 1
fi
if ! grep -Fq "'@fontsource-variable/inter@5.2.8':" pnpm-lock.yaml; then
  echo "release asset inventory: pnpm lock does not contain @fontsource-variable/inter@5.2.8" >&2
  exit 1
fi

pow_license_sha="$(sha256sum desktop/public/pow/LICENSE.txt | awk '{print $1}')"
if [[ "${pow_license_sha}" != "bf8a18984e613f9a1d412cbf6ed6a2845447dd0ae9553ebfde6360aa518abd9b" ]]; then
  echo "release asset inventory: Emerge Tools Pow license text changed" >&2
  exit 1
fi

# A status string cannot clear a release obligation by itself. Every cleared
# obligation must point at a tracked, tag-scoped evidence record whose bytes,
# schema, release tag, obligation ID, and source commit all agree.
while IFS=$'\t' read -r obligation_id evidence_schema evidence_path evidence_sha release_tag; do
  expected_path="docs/release/evidence/${release_tag}/${obligation_id}.json"
  if [[ "${evidence_path}" != "${expected_path}" ]]; then
    echo "release asset inventory: ${obligation_id} evidence path must be ${expected_path}" >&2
    exit 1
  fi
  if [[ "${evidence_schema}" != "${release_obligation_evidence_schema}" ]]; then
    echo "release asset inventory: ${obligation_id} evidence schema is ${evidence_schema}, expected ${release_obligation_evidence_schema}" >&2
    exit 1
  fi
  if [[ ! -f "${evidence_path}" ]]; then
    echo "release asset inventory: ${obligation_id} evidence does not exist: ${evidence_path}" >&2
    exit 1
  fi
  if [[ -L "${evidence_path}" ]]; then
    echo "release asset inventory: ${obligation_id} evidence must not be a symbolic link: ${evidence_path}" >&2
    exit 1
  fi
  if ! git ls-files --error-unmatch -- "${evidence_path}" >/dev/null 2>&1; then
    echo "release asset inventory: ${obligation_id} evidence is not tracked: ${evidence_path}" >&2
    exit 1
  fi

  actual_evidence_sha="$(sha256sum "${evidence_path}" | awk '{print $1}')"
  if [[ "${actual_evidence_sha}" != "${evidence_sha}" ]]; then
    echo "release asset inventory: ${obligation_id} evidence content changed" >&2
    echo "  expected sha256: ${evidence_sha}" >&2
    echo "  actual sha256:   ${actual_evidence_sha}" >&2
    exit 1
  fi

  jq -e \
    --arg schema "${release_obligation_evidence_schema}" \
    --arg obligation_id "${obligation_id}" \
    --arg release_tag "${release_tag}" '
      .schema == $schema
      and .obligation_id == $obligation_id
      and .release_tag == $release_tag
      and (.source_commit | type == "string" and test("^[0-9a-f]{40}$"))
      and .result == "passed"
      and (.recorded_at | type == "string" and test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"))
      and (.checks | type == "array" and length > 0)
      and all(
        .checks[];
        (.id | type == "string" and length > 0)
        and .status == "passed"
        and (.evidence | type == "string" and length > 0)
      )
    ' "${evidence_path}" >/dev/null || {
    echo "release asset inventory: ${obligation_id} evidence has an invalid record format" >&2
    exit 1
  }

  evidence_commit="$(jq -r '.source_commit' "${evidence_path}")"
  if git rev-parse --verify --quiet "refs/tags/${release_tag}^{commit}" >/dev/null; then
    tag_commit="$(git rev-parse "refs/tags/${release_tag}^{commit}")"
    if [[ "${tag_commit}" != "${evidence_commit}" ]]; then
      echo "release asset inventory: ${obligation_id} evidence source_commit does not match ${release_tag}" >&2
      exit 1
    fi
  elif [[ "${release_mode}" == true ]]; then
    echo "release asset inventory: release tag is unavailable for ${obligation_id} evidence: ${release_tag}" >&2
    exit 1
  fi

  if [[ "${release_mode}" == true ]]; then
    head_commit="$(git rev-parse HEAD)"
    if [[ "${head_commit}" != "${evidence_commit}" ]]; then
      echo "release asset inventory: ${obligation_id} evidence is not bound to the release HEAD" >&2
      exit 1
    fi
  fi
done < <(
  jq -r '
    .release_obligations[]
    | select(.release_status == "cleared")
    | [.id, .evidence.schema, .evidence.path, .evidence.sha256, .evidence.release_tag]
    | @tsv
  ' "${manifest}"
)

mapfile -t blockers < <(
  jq -r '
    [
      .filesystem_assets[],
      .font_dependencies[],
      .packaged_programs[],
      .release_obligations[]
    ]
    | .[]
    | select(.release_status == "blocked")
    | .id
  ' "${manifest}"
)

echo "release asset inventory: integrity checks passed"
if [[ ${#blockers[@]} -gt 0 ]]; then
  echo "release asset inventory: ${#blockers[@]} release blocker(s) remain:"
  printf '  - %s\n' "${blockers[@]}"
  if [[ "${release_mode}" == true ]]; then
    echo "release asset inventory: refusing release while blockers remain" >&2
    exit 1
  fi
else
  echo "release asset inventory: no blockers remain"
fi
