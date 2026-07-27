#!/usr/bin/env bash
# Run Project View's Postgres-backed transaction tests. Every selected test
# creates and drops its own database; the configured database is used only as
# an administrative connection.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

if [[ -z "${PROJECT_VIEW_TEST_DATABASE_URL:-}" ]]; then
  docker compose up -d postgres >/dev/null
  for _ in $(seq 1 60); do
    status="$(docker inspect --format='{{.State.Health.Status}}' buzz-postgres 2>/dev/null || true)"
    [[ "${status}" == "healthy" ]] && break
    sleep 2
  done
  if [[ "${status:-}" != "healthy" ]]; then
    docker logs buzz-postgres || true
    echo "Project View DB tests: Postgres did not become healthy" >&2
    exit 1
  fi
  PROJECT_VIEW_TEST_DATABASE_URL="postgres://buzz:buzz_dev@localhost:5432/postgres"
fi

export TEST_DATABASE_URL="${PROJECT_VIEW_TEST_DATABASE_URL}"

if [[ -n "${PROJECT_VIEW_TEST_ARCHIVE:-}" ]]; then
  cargo nextest run \
    --archive-file "${PROJECT_VIEW_TEST_ARCHIVE}" \
    -E 'package(buzz-db) and test(project_view)' \
    --run-ignored ignored-only \
    --test-threads 1
elif command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run \
    -p buzz-db \
    --lib \
    -E 'test(project_view)' \
    --run-ignored ignored-only \
    --test-threads 1
else
  cargo test -p buzz-db --lib project_view -- \
    --ignored \
    --nocapture \
    --test-threads=1
fi
