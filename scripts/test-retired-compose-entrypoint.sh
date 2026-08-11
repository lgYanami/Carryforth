#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ENTRYPOINT="$REPO_ROOT/deploy/compose/run.sh"
COMPOSE_FILE="$REPO_ROOT/deploy/compose/compose.yml"

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf -- "$tmp_dir"
}
trap cleanup EXIT HUP INT TERM

cat >"$tmp_dir/docker" <<'SH'
#!/usr/bin/env bash
echo "unexpected docker invocation: $*" >>"${RETIRED_COMPOSE_DOCKER_LOG:?}"
exit 99
SH
chmod +x "$tmp_dir/docker"

export RETIRED_COMPOSE_DOCKER_LOG="$tmp_dir/docker.log"
: >"$RETIRED_COMPOSE_DOCKER_LOG"

help_output="$(PATH="$tmp_dir:$PATH" "$ENTRYPOINT" help)"
if [[ "$help_output" != *"deploy/compose is retired"* ]] ||
  [[ "$help_output" != *"deploy/local"* ]]; then
  echo "retired Compose help does not point to deploy/local" >&2
  exit 1
fi

for command in start up stop down restart pull upgrade logs status ps config backup-hint add-member remove-member list-members; do
  set +e
  output="$(PATH="$tmp_dir:$PATH" "$ENTRYPOINT" "$command" 2>&1)"
  status=$?
  set -e
  if [[ $status -ne 78 ]]; then
    echo "retired Compose command '$command' returned $status instead of 78" >&2
    exit 1
  fi
  if [[ "$output" != *"deploy/compose is retired"* ]] ||
    [[ "$output" != *"deploy/local"* ]]; then
    echo "retired Compose command '$command' omitted the migration destination" >&2
    exit 1
  fi
done

if [[ -s "$RETIRED_COMPOSE_DOCKER_LOG" ]]; then
  echo "retired Compose entrypoint invoked Docker" >&2
  cat "$RETIRED_COMPOSE_DOCKER_LOG" >&2
  exit 1
fi

if ! rg -q '^services:[[:space:]]*\{\}[[:space:]]*$' "$COMPOSE_FILE"; then
  echo "retired compose.yml is not an empty service tombstone" >&2
  exit 1
fi

if rg -q --ignore-case 'ghcr\.io/block|image:[[:space:]]*[^#[:space:]]+:(main|latest|master|edge|nightly|dev)([[:space:]#]|$)' \
  "$REPO_ROOT/deploy/compose/compose.yml" \
  "$REPO_ROOT/deploy/compose/.env.example" \
  "$REPO_ROOT/deploy/local/compose.yml"; then
  echo "active deployment files contain a Block image or floating image tag" >&2
  exit 1
fi

echo "Retired Compose entrypoint contract passed."
