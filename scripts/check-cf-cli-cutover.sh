#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "${repo_root}"

failed=0

reject_matches() {
  local description="$1"
  local pattern="$2"
  shift 2

  local matches
  if matches="$(rg -n --pcre2 "${pattern}" "$@" 2>/dev/null)"; then
    printf 'cf cutover check failed: %s\n%s\n' "${description}" "${matches}" >&2
    failed=1
  fi
}

if [[ -e crates/buzz-cli ]]; then
  echo "cf cutover check failed: retired crates/buzz-cli still exists" >&2
  failed=1
fi

reject_matches \
  "the retired Cargo package, library, or standalone CLI path is still referenced" \
  '(^|[^[:alnum:]_-])(buzz-cli|buzz_cli|crates/buzz-cli)([^[:alnum:]_-]|$)|target/(debug|release|ci)/buzz([^[:alnum:]_-]|$)' \
  Cargo.toml Cargo.lock Justfile .github/workflows scripts crates desktop AGENTS.md README.md TESTING.md \
  --glob '!scripts/check-cf-cli-cutover.sh' --glob '!docs/**' \
  --glob '!desktop/src-tauri/src/managed_agents/nest.rs' \
  --glob '!desktop/src-tauri/src/managed_agents/nest/tests.rs' \
  --glob '!**/target/**' --glob '!desktop/node_modules/**'

reject_matches \
  "the Carryforth CLI still reads a retired BUZZ_* public identity variable" \
  'BUZZ_(RELAY_URL|PRIVATE_KEY|AUTH_TAG|CONNECT_TIMEOUT_SECS|TIMEOUT_SECS|CLI_TEST_DURATION_SECS)' \
  crates/carryforth-cli

reject_matches \
  "Desktop still packages the retired buzz CLI sidecar" \
  'binaries/buzz(["[:space:]]|$)|binaries/buzz-\$TARGET|git-credential-nostr[[:space:]]+buzz([^[:alnum:]_-]|$)' \
  desktop/src-tauri/tauri.conf.json Justfile scripts/bundle-sidecars.sh .github/workflows

reject_matches \
  "the developer MCP still exposes a buzz multicall personality" \
  '(join\("buzz"\)|cmd\s*==\s*"buzz"|multicall[^\n]*"buzz")' \
  crates/buzz-dev-mcp/src

reject_matches \
  "a current Human/Agent-facing surface still emits an actionable buzz CLI command" \
  '(?<![-[:alnum:]_])buzz[[:space:]]+(messages|channels|dms|reactions|canvas|feed|users|workflows|social|repos|upload|mem|notes|patches|pr|issues|emoji|pack|agents|project-view|project-context|documents|roles|resources|meetings|moderation)([^-[:alnum:]_]|$)' \
  crates/buzz-acp/src crates/buzz-agent/src crates/buzz-dev-mcp/src crates/buzz-sdk/src \
  crates/carryforth-cli desktop/src desktop/src-tauri/src scripts \
  AGENTS.md README.md TESTING.md \
  --glob '!**/target/**' --glob '!desktop/node_modules/**'

if (( failed != 0 )); then
  exit 1
fi

echo "cf CLI cutover check passed"
