#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SOURCE_DIR="$REPO_ROOT/deploy/local"
TEMP_ROOT="$(mktemp -d)"
trap 'find "$TEMP_ROOT" -depth -delete' EXIT

TEST_DIR="$TEMP_ROOT/deploy/local"
mkdir -p "$TEST_DIR"
cp "$SOURCE_DIR/run.sh" "$SOURCE_DIR/compose.yml" "$TEST_DIR/"

bash -n "$TEST_DIR/run.sh"
"$TEST_DIR/run.sh" init --image ghcr.io/lgyanami/carryforth-relay:0.1.0 >/dev/null
"$TEST_DIR/run.sh" validate >/dev/null

[[ "$(stat -c '%a' "$TEST_DIR/.env")" == "600" ]]
! rg --quiet 'CHANGE_ME|:(latest|main)([[:space:]]|$)' "$TEST_DIR/.env"
[[ "$(rg -c '^CARRYFORTH_RELAY_IMAGE=' "$TEST_DIR/.env")" == "1" ]]
! rg --quiet 'source[[:space:]]+.*ENV_FILE' "$TEST_DIR/run.sh"

if "$TEST_DIR/run.sh" init --image ghcr.io/lgyanami/carryforth-relay:0.1.0 \
  >/dev/null 2>&1; then
  echo "init unexpectedly overwrote an existing .env" >&2
  exit 1
fi

git -C "$REPO_ROOT" check-ignore --quiet deploy/local/.env.tmp.example

CONCURRENT_DIR="$TEMP_ROOT/concurrent-init"
mkdir -p "$CONCURRENT_DIR"
cp "$SOURCE_DIR/run.sh" "$SOURCE_DIR/compose.yml" "$CONCURRENT_DIR/"
pids=()
for attempt in 1 2; do
  "$CONCURRENT_DIR/run.sh" init --image ghcr.io/lgyanami/carryforth-relay:0.1.0 \
    >"$TEMP_ROOT/concurrent-$attempt.log" 2>&1 &
  pids+=("$!")
done
successes=0
for pid in "${pids[@]}"; do
  if wait "$pid"; then
    ((successes += 1))
  fi
done
[[ "$successes" == "1" ]]
"$CONCURRENT_DIR/run.sh" validate >/dev/null
if compgen -G "$CONCURRENT_DIR/.env.tmp.*" >/dev/null; then
  echo "concurrent init left a secret-bearing temporary file" >&2
  exit 1
fi

for rejected in \
  ghcr.io/block/buzz:v0.4.26 \
  ghcr.io/example/carryforth-relay:v0.1.0 \
  ghcr.io/lgyanami/carryforth-relay:main \
  ghcr.io/lgyanami/carryforth-relay:latest \
  ghcr.io/lgyanami/carryforth-relay:v0.1.0 \
  ghcr.io/lgyanami/carryforth-relay:v01.2.3 \
  ghcr.io/lgyanami/carryforth-relay@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  ghcr.io/lgyanami/carryforth-relay@sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA; do
  isolated="$TEMP_ROOT/rejected-$(printf '%s' "$rejected" | tr '/:' '--')"
  mkdir -p "$isolated"
  cp "$SOURCE_DIR/run.sh" "$SOURCE_DIR/compose.yml" "$isolated/"
  if "$isolated/run.sh" init --image "$rejected" >/dev/null 2>&1; then
    echo "init accepted a forbidden image: $rejected" >&2
    exit 1
  fi
done

DIGEST_DIR="$TEMP_ROOT/digest-init"
mkdir -p "$DIGEST_DIR"
cp "$SOURCE_DIR/run.sh" "$SOURCE_DIR/compose.yml" "$DIGEST_DIR/"
RELAY_DIGEST="sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"$DIGEST_DIR/run.sh" init \
  --image "ghcr.io/lgyanami/carryforth-relay:0.1.0@$RELAY_DIGEST" >/dev/null
"$DIGEST_DIR/run.sh" validate >/dev/null
rg --quiet \
  "^CARRYFORTH_RELAY_IMAGE=ghcr.io/lgyanami/carryforth-relay:0\\.1\\.0@$RELAY_DIGEST$" \
  "$DIGEST_DIR/.env"

cp "$TEST_DIR/.env" "$TEST_DIR/.env.clean"
printf 'CARRYFORTH_RELAY_IMAGE=ghcr.io/lgyanami/carryforth-relay:0.1.0\n' >>"$TEST_DIR/.env"
if "$TEST_DIR/run.sh" validate >/dev/null 2>&1; then
  echo "validation accepted a duplicate Relay image coordinate" >&2
  exit 1
fi
mv "$TEST_DIR/.env.clean" "$TEST_DIR/.env"

cp "$TEST_DIR/.env" "$TEST_DIR/.env.clean"
printf 'UNREVIEWED_OPTION=value\n' >>"$TEST_DIR/.env"
if "$TEST_DIR/run.sh" validate >/dev/null 2>&1; then
  echo "validation accepted an unknown local-stack option" >&2
  exit 1
fi
mv "$TEST_DIR/.env.clean" "$TEST_DIR/.env"

rg --quiet 'RELAY_URL:[[:space:]]+ws://localhost:3000' "$TEST_DIR/compose.yml"
! rg --quiet 'BUZZ_RELAY_URL:' "$TEST_DIR/compose.yml"
rg --quiet 'BUZZ_MEETING_V2_CREATE_ENABLED:[[:space:]]+"true"' "$TEST_DIR/compose.yml"
rg --quiet 'BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED:[[:space:]]+"true"' "$TEST_DIR/compose.yml"
rg --quiet 'BUZZ_MEETING_COMMUNITY_READ_ENABLED:[[:space:]]+"true"' "$TEST_DIR/compose.yml"
rg --quiet 'install -d -o buzz -g buzz -m 0750 /data/git' "$REPO_ROOT/Dockerfile"
rg --quiet 'SocketAddr::new\(config\.bind_addr\.ip\(\), config\.health_port\)' \
  "$REPO_ROOT/crates/buzz-relay/src/main.rs"
rg --quiet 'SocketAddr::new\(config\.bind_addr\.ip\(\), config\.metrics_port\)' \
  "$REPO_ROOT/crates/buzz-relay/src/main.rs"

if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1 &&
  docker compose version >/dev/null 2>&1; then
  rendered="$TEMP_ROOT/rendered.yml"
  docker compose --env-file "$TEST_DIR/.env" -f "$TEST_DIR/compose.yml" config >"$rendered"
  if rg --ignore-case 'push\.buzz|builderlab|keycloak|prometheus|ghcr\.io/block' "$rendered"; then
    echo "rendered local Compose config contains a retired or optional service" >&2
    exit 1
  fi

  postgres_password="$(awk -F= '$1 == "POSTGRES_PASSWORD" { print $2 }' "$TEST_DIR/.env")"
  config_output="$TEMP_ROOT/config-output.yml"
  "$TEST_DIR/run.sh" config >"$config_output"
  if rg --fixed-strings --quiet "$postgres_password" "$config_output"; then
    echo "run.sh config exposed a generated secret" >&2
    exit 1
  fi
  rg --fixed-strings --quiet '${POSTGRES_PASSWORD:?missing POSTGRES_PASSWORD}' "$config_output"
fi

# Exercise upgrade failure handling without touching the host daemon. Once the
# new binary may have started, the script must stop it and must not launch the
# old binary against a potentially migrated database.
FAKE_BIN="$TEMP_ROOT/fake-bin"
mkdir -p "$FAKE_BIN"
cat >"$FAKE_BIN/docker" <<'FAKE_DOCKER'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$CARRYFORTH_TEST_DOCKER_LOG"
[[ "${1:-}" == "info" ]] && exit 0
[[ "${1:-}" == "compose" ]] || exit 1
shift
[[ "${1:-}" == "version" ]] && exit 0

env_file=""
args=("$@")
for ((index = 0; index < ${#args[@]}; index += 1)); do
  if [[ "${args[$index]}" == "--env-file" ]]; then
    env_file="${args[$((index + 1))]}"
    break
  fi
done
[[ -n "$env_file" ]] || exit 1
image="$(awk -F= '$1 == "CARRYFORTH_RELAY_IMAGE" { print $2 }' "$env_file")"
if [[ " $* " == *" pull relay " && "${CARRYFORTH_TEST_FAIL_PULL:-0}" == "1" ]]; then
  exit 1
fi
if [[ " $* " == *" up -d --wait relay "* && "$image" == *":0.2.0" ]]; then
  exit 1
fi
exit 0
FAKE_DOCKER
chmod +x "$FAKE_BIN/docker"
FAKE_LOG="$TEMP_ROOT/fake-docker.log"

# Downgrades, including stable-to-prerelease transitions, fail before any
# image pull or container replacement is attempted.
sed -i \
  's#^CARRYFORTH_RELAY_IMAGE=.*#CARRYFORTH_RELAY_IMAGE=ghcr.io/lgyanami/carryforth-relay:0.2.0#' \
  "$TEST_DIR/.env"
: >"$FAKE_LOG"
if CARRYFORTH_TEST_DOCKER_LOG="$FAKE_LOG" PATH="$FAKE_BIN:$PATH" \
  "$TEST_DIR/run.sh" upgrade --image ghcr.io/lgyanami/carryforth-relay:0.1.0 \
  >"$TEMP_ROOT/downgrade-output" 2>&1; then
  echo "upgrade unexpectedly accepted a Relay downgrade" >&2
  exit 1
fi
rg --quiet 'downgrade.*forbidden' "$TEMP_ROOT/downgrade-output"
! rg --quiet 'pull relay|up -d --wait relay' "$FAKE_LOG"

: >"$FAKE_LOG"
if CARRYFORTH_TEST_DOCKER_LOG="$FAKE_LOG" PATH="$FAKE_BIN:$PATH" \
  "$TEST_DIR/run.sh" upgrade --image ghcr.io/lgyanami/carryforth-relay:0.2.0-rc.2 \
  >"$TEMP_ROOT/prerelease-downgrade-output" 2>&1; then
  echo "upgrade unexpectedly accepted a stable-to-prerelease downgrade" >&2
  exit 1
fi
rg --quiet 'downgrade.*forbidden' "$TEMP_ROOT/prerelease-downgrade-output"
! rg --quiet 'pull relay|up -d --wait relay' "$FAKE_LOG"

# The only same-version replacement allowed is strengthening a mutable tag to
# the release-qualified digest for that exact version.
SAME_VERSION_DIGEST="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
: >"$FAKE_LOG"
CARRYFORTH_TEST_DOCKER_LOG="$FAKE_LOG" PATH="$FAKE_BIN:$PATH" \
  "$TEST_DIR/run.sh" upgrade \
  --image "ghcr.io/lgyanami/carryforth-relay:0.2.0@$SAME_VERSION_DIGEST" \
  >"$TEMP_ROOT/digest-strengthening-output" 2>&1
rg --quiet \
  "^CARRYFORTH_RELAY_IMAGE=ghcr.io/lgyanami/carryforth-relay:0\\.2\\.0@$SAME_VERSION_DIGEST$" \
  "$TEST_DIR/.env"
[[ "$(rg -c 'pull relay' "$FAKE_LOG")" == "1" ]]
[[ "$(rg -c 'up -d --wait relay' "$FAKE_LOG")" == "1" ]]

sed -i \
  's#^CARRYFORTH_RELAY_IMAGE=.*#CARRYFORTH_RELAY_IMAGE=ghcr.io/lgyanami/carryforth-relay:0.1.0#' \
  "$TEST_DIR/.env"
: >"$FAKE_LOG"
if CARRYFORTH_TEST_DOCKER_LOG="$FAKE_LOG" PATH="$FAKE_BIN:$PATH" \
  "$TEST_DIR/run.sh" upgrade --image ghcr.io/lgyanami/carryforth-relay:0.2.0 \
  >"$TEMP_ROOT/upgrade-output" 2>&1; then
  echo "upgrade unexpectedly succeeded when the new Relay failed readiness" >&2
  exit 1
fi
rg --quiet '^CARRYFORTH_RELAY_IMAGE=ghcr.io/lgyanami/carryforth-relay:0\.2\.0$' "$TEST_DIR/.env"
rg --quiet 'Do not start the old image' "$TEMP_ROOT/upgrade-output"
[[ "$(rg -c 'up -d --wait relay' "$FAKE_LOG")" == "1" ]]
rg --quiet 'stop relay' "$FAKE_LOG"

# A pull failure happens before a new binary can migrate data, so restoring
# and starting the previous image is safe.
sed -i \
  's#^CARRYFORTH_RELAY_IMAGE=.*#CARRYFORTH_RELAY_IMAGE=ghcr.io/lgyanami/carryforth-relay:0.1.0#' \
  "$TEST_DIR/.env"
: >"$FAKE_LOG"
if CARRYFORTH_TEST_FAIL_PULL=1 CARRYFORTH_TEST_DOCKER_LOG="$FAKE_LOG" PATH="$FAKE_BIN:$PATH" \
  "$TEST_DIR/run.sh" upgrade --image ghcr.io/lgyanami/carryforth-relay:0.3.0 \
  >"$TEMP_ROOT/pull-failure-output" 2>&1; then
  echo "upgrade unexpectedly succeeded when the new Relay pull failed" >&2
  exit 1
fi
rg --quiet '^CARRYFORTH_RELAY_IMAGE=ghcr.io/lgyanami/carryforth-relay:0\.1\.0$' "$TEST_DIR/.env"
rg --quiet 'previous Relay service is running' "$TEMP_ROOT/pull-failure-output"
[[ "$(rg -c 'up -d --wait relay' "$FAKE_LOG")" == "1" ]]

echo "Carryforth local deployment contract tests passed."
