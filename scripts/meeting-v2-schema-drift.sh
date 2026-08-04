#!/usr/bin/env bash
# Fail when migration-built Meeting objects differ from schema/schema.sql.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PLAN_FILE="$(mktemp)"

cleanup() {
  rm -f "${PLAN_FILE}"
}
trap cleanup EXIT

for required in PGHOST PGPORT PGUSER PGPASSWORD PGDATABASE; do
  if [[ -z "${!required:-}" ]]; then
    echo "${required} is required for the Meeting schema-drift gate" >&2
    exit 2
  fi
done

cd "${REPO_ROOT}"
PGSCHEMA_PLAN_HOST="${PGSCHEMA_PLAN_HOST:-${PGHOST}}" \
PGSCHEMA_PLAN_PORT="${PGSCHEMA_PLAN_PORT:-${PGPORT}}" \
PGSCHEMA_PLAN_DB="${PGSCHEMA_PLAN_DB:-postgres}" \
PGSCHEMA_PLAN_USER="${PGSCHEMA_PLAN_USER:-${PGUSER}}" \
PGSCHEMA_PLAN_PASSWORD="${PGSCHEMA_PLAN_PASSWORD:-${PGPASSWORD}}" \
  ./bin/pgschema plan \
    --file schema/schema.sql \
    --output-json "${PLAN_FILE}" \
    --no-color >/dev/null

meeting_schema_drift="$(
  jq '[
    .groups[].steps[]?
    | select(
        ((.path // "") | test("meeting_"; "i"))
        or ((.sql // "") | test("meeting_"; "i"))
      )
  ]' "${PLAN_FILE}"
)"
if [[ "$(jq 'length' <<<"${meeting_schema_drift}")" != "0" ]]; then
  echo "Meeting migration/schema drift detected:" >&2
  jq . <<<"${meeting_schema_drift}" >&2
  exit 1
fi

echo "Meeting migration/schema-drift gate passed."
