#!/usr/bin/env bash
# Run Project Document's PostgreSQL-backed transaction/race tests plus the
# shared managed-owner reader and supervised-runtime fences it relies on. Every
# selected test creates and drops its own database; the configured database is
# used only as an administrative connection.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

if [[ -z "${PROJECT_DOCUMENT_TEST_DATABASE_URL:-}" ]]; then
  docker compose up -d postgres >/dev/null
  for _ in $(seq 1 60); do
    status="$(docker inspect --format='{{.State.Health.Status}}' buzz-postgres 2>/dev/null || true)"
    [[ "${status}" == "healthy" ]] && break
    sleep 2
  done
  if [[ "${status:-}" != "healthy" ]]; then
    docker logs buzz-postgres || true
    echo "Project Document DB tests: Postgres did not become healthy" >&2
    exit 1
  fi
  PROJECT_DOCUMENT_TEST_DATABASE_URL="postgres://buzz:buzz_dev@localhost:5432/postgres"
fi

export TEST_DATABASE_URL="${PROJECT_DOCUMENT_TEST_DATABASE_URL}"

if [[ -n "${PROJECT_DOCUMENT_TEST_ARCHIVE:-}" ]]; then
  cargo nextest run \
    --archive-file "${PROJECT_DOCUMENT_TEST_ARCHIVE}" \
    -E 'package(buzz-db) and (test(project_document) or test(strict_reader_gate_accepts_members_and_owned_agents_and_honors_bans) or test(trusted_runtime_supervision_fails_closed_and_commits_one_system_change))' \
    --run-ignored ignored-only \
    --test-threads 1
elif command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run \
    -p buzz-db \
    --lib \
    -E 'test(project_document) or test(strict_reader_gate_accepts_members_and_owned_agents_and_honors_bans) or test(trusted_runtime_supervision_fails_closed_and_commits_one_system_change)' \
    --run-ignored ignored-only \
    --test-threads 1
else
  cargo test -p buzz-db --lib project_document -- \
    --ignored \
    --nocapture \
    --test-threads=1
  cargo test -p buzz-db --lib strict_reader_gate_accepts_members_and_owned_agents_and_honors_bans -- \
    --ignored \
    --nocapture \
    --test-threads=1
  cargo test -p buzz-db --lib trusted_runtime_supervision_fails_closed_and_commits_one_system_change -- \
    --ignored \
    --nocapture \
    --test-threads=1
fi
