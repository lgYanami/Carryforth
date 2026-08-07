#!/usr/bin/env bash
# Run Project Context Edge's PostgreSQL-backed canonical storage tests. Each
# selected test creates and drops its own database; the configured database is
# used only as an administrative connection.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

admin_database_name=""
cleanup() {
  if [[ -n "${admin_database_name}" ]]; then
    docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
      psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS ${admin_database_name} WITH (FORCE)" >/dev/null
  fi
}
trap cleanup EXIT

if [[ -z "${BUZZ_TEST_DATABASE_URL:-}" ]]; then
  docker compose up -d postgres >/dev/null
  status=""
  for _ in $(seq 1 60); do
    status="$(docker inspect --format='{{.State.Health.Status}}' buzz-postgres 2>/dev/null || true)"
    [[ "${status}" == "healthy" ]] && break
    sleep 2
  done
  if [[ "${status}" != "healthy" ]]; then
    docker logs buzz-postgres || true
    echo "Project Context DB tests: Postgres did not become healthy" >&2
    exit 1
  fi
  admin_database_name="buzz_pc_admin_$$_${RANDOM}"
  if [[ ! "${admin_database_name}" =~ ^buzz_pc_admin_[0-9_]+$ ]]; then
    echo "Refusing unsafe Project Context administrative database name: ${admin_database_name}" >&2
    exit 1
  fi
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "CREATE DATABASE ${admin_database_name}" >/dev/null
  BUZZ_TEST_DATABASE_URL="postgres://buzz:buzz_dev@localhost:5432/${admin_database_name}"
fi

configured_database="${BUZZ_TEST_DATABASE_URL%%\?*}"
configured_database="${configured_database##*/}"
if [[ ! "${configured_database}" =~ ^buzz_ ]]; then
  echo "Project Context DB tests require BUZZ_TEST_DATABASE_URL to name a disposable buzz_ database" >&2
  exit 1
fi
export BUZZ_TEST_DATABASE_URL

if [[ -n "${PROJECT_CONTEXT_TEST_ARCHIVE:-}" ]]; then
  cargo nextest run \
    --archive-file "${PROJECT_CONTEXT_TEST_ARCHIVE}" \
    -E 'package(buzz-db) and test(project_context)' \
    --run-ignored ignored-only \
    --test-threads 1
elif command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run \
    -p buzz-db \
    --lib \
    -E 'test(project_context)' \
    --run-ignored ignored-only \
    --test-threads 1
else
  cargo test -p buzz-db --lib project_context -- \
    --ignored \
    --nocapture \
    --test-threads=1
fi
