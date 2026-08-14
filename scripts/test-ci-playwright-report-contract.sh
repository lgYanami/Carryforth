#!/usr/bin/env bash
# Freeze the Playwright JSON-report wiring and exercise strict report parsing.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKFLOW="${REPO_ROOT}/.github/workflows/ci.yml"
PLAYWRIGHT_CONFIG="${REPO_ROOT}/desktop/playwright.config.ts"
SUMMARIZER="${REPO_ROOT}/desktop/scripts/summarize-flaky-tests.mjs"

require_literal() {
  local file="$1"
  local literal="$2"
  local description="$3"

  if ! grep -Fq -- "${literal}" "${file}"; then
    printf 'Missing %s in %s\n' "${description}" "${file}" >&2
    exit 1
  fi
}

require_count_at_least() {
  local file="$1"
  local literal="$2"
  local minimum="$3"
  local description="$4"
  local count

  count="$(grep -Fc -- "${literal}" "${file}" || true)"
  if (( count < minimum )); then
    printf 'Expected at least %d occurrence(s) of %s in %s; found %d\n' \
      "${minimum}" "${description}" "${file}" "${count}" >&2
    exit 1
  fi
}

expect_failure() {
  local description="$1"
  shift

  if "$@" >/dev/null 2>&1; then
    printf 'Expected failure: %s\n' "${description}" >&2
    exit 1
  fi
}

require_literal \
  "${PLAYWRIGHT_CONFIG}" \
  '["json", { outputFile: "playwright-report.json" }]' \
  'Playwright JSON reporter'
require_count_at_least \
  "${WORKFLOW}" \
  'node scripts/summarize-flaky-tests.mjs playwright-report.json' \
  2 \
  'smoke and integration report summarizers'
require_count_at_least \
  "${WORKFLOW}" \
  '--strict --output flaky-summary.md' \
  2 \
  'strict report validation'
require_count_at_least \
  "${WORKFLOW}" \
  'desktop/playwright-report.json' \
  2 \
  'uploaded Playwright JSON evidence'
require_count_at_least \
  "${WORKFLOW}" \
  'desktop/flaky-summary.md' \
  2 \
  'uploaded flaky-test summaries'
require_count_at_least \
  "${WORKFLOW}" \
  "steps.playwright-report-ready.outcome == 'success'" \
  4 \
  'report validation and upload execution fences'

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

printf '{"suites":[]}\n' >"${tmp}/valid.json"
node "${SUMMARIZER}" \
  "${tmp}/valid.json" \
  'contract-valid' \
  --strict \
  --output "${tmp}/valid.md" \
  >/dev/null
test -s "${tmp}/valid.md"
grep -Fq 'No test passed only after a retry.' "${tmp}/valid.md"

printf '%s\n' \
  '{"suites":[{"file":"fixture.spec.ts","specs":[{"title":"recovers","tests":[{"status":"flaky","projectName":"smoke","results":[{},{}]}]}]}]}' \
  >"${tmp}/flaky.json"
node "${SUMMARIZER}" \
  "${tmp}/flaky.json" \
  'contract-flaky' \
  --strict \
  --output "${tmp}/flaky.md" \
  >/dev/null
grep -Fq '| fixture.spec.ts › recovers | smoke | 2 |' "${tmp}/flaky.md"

printf '{not-json\n' >"${tmp}/malformed.json"
expect_failure \
  'strict mode must reject malformed JSON' \
  node "${SUMMARIZER}" "${tmp}/malformed.json" malformed --strict

printf '{"config":{}}\n' >"${tmp}/missing-suites.json"
expect_failure \
  'strict mode must reject reports without suites' \
  node "${SUMMARIZER}" "${tmp}/missing-suites.json" missing-suites --strict

: >"${tmp}/empty.json"
expect_failure \
  'strict mode must reject an empty report' \
  node "${SUMMARIZER}" "${tmp}/empty.json" empty --strict
expect_failure \
  'strict mode must reject a missing report' \
  node "${SUMMARIZER}" "${tmp}/absent.json" absent --strict

node "${SUMMARIZER}" "${tmp}/absent.json" non-strict >/dev/null
expect_failure \
  'unknown options must fail closed' \
  node "${SUMMARIZER}" "${tmp}/valid.json" unknown --not-an-option

echo 'CI Playwright report contract passed.'
