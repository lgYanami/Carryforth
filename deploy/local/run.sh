#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$SCRIPT_DIR/.env"
COMPOSE_FILE="$SCRIPT_DIR/compose.yml"

readonly ENV_KEYS=(
  CARRYFORTH_RELAY_IMAGE
  CARRYFORTH_POSTGRES_PORT
  CARRYFORTH_REDIS_PORT
  CARRYFORTH_MINIO_PORT
  CARRYFORTH_HEALTH_PORT
  CARRYFORTH_METRICS_PORT
  POSTGRES_DB
  POSTGRES_USER
  POSTGRES_PASSWORD
  REDIS_PASSWORD
  CARRYFORTH_S3_ACCESS_KEY
  CARRYFORTH_S3_SECRET_KEY
  CARRYFORTH_S3_BUCKET
  CARRYFORTH_RELAY_PRIVATE_KEY
  CARRYFORTH_GIT_HOOK_HMAC_SECRET
)

fail() {
  printf '[carryforth-local] ERROR: %s\n' "$*" >&2
  exit 1
}

log() {
  printf '[carryforth-local] %s\n' "$*"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

require_runtime() {
  require_command docker
  docker info >/dev/null 2>&1 || fail "Docker daemon is not running"
  docker compose version >/dev/null 2>&1 || fail "Docker Compose v2 is required"
}

require_linux() {
  [[ "$(uname -s)" == "Linux" ]] || fail "the first public local stack supports Linux only"
}

is_known_env_key() {
  local candidate="$1"
  local known
  for known in "${ENV_KEYS[@]}"; do
    [[ "$candidate" == "$known" ]] && return 0
  done
  return 1
}

read_env_value() {
  local key="$1"
  local value
  if ! value="$(awk -v key="$key" '
    index($0, key "=") == 1 {
      count += 1
      value = substr($0, length(key) + 2)
    }
    END {
      if (count != 1) exit 1
      print value
    }
  ' "$ENV_FILE")"; then
    fail "$key must appear exactly once in $ENV_FILE"
  fi
  [[ -n "$value" ]] || fail "$key must not be empty in $ENV_FILE"
  printf '%s\n' "$value"
}

validate_env_file() {
  [[ -f "$ENV_FILE" && ! -L "$ENV_FILE" ]] ||
    fail "missing or unsafe deploy/local/.env; run ./run.sh init --image <pinned-image>"
  local env_mode
  env_mode="$(stat -c '%a' "$ENV_FILE")"
  (( (8#$env_mode & 077) == 0 )) ||
    fail "$ENV_FILE must not grant group or world permissions"

  local -A seen=()
  local line key value
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    [[ "$line" == *=* ]] || fail "$ENV_FILE contains an invalid line"
    key="${line%%=*}"
    value="${line#*=}"
    [[ "$key" =~ ^[A-Z][A-Z0-9_]*$ ]] || fail "$ENV_FILE contains an invalid key"
    is_known_env_key "$key" || fail "$ENV_FILE contains an unsupported key: $key"
    [[ -z "${seen[$key]:-}" ]] || fail "$key must appear exactly once in $ENV_FILE"
    [[ -n "$value" && "$value" =~ ^[A-Za-z0-9_./:@+-]+$ ]] ||
      fail "$key contains an empty or unsupported value"
    seen[$key]=1
  done <"$ENV_FILE"

  for key in "${ENV_KEYS[@]}"; do
    [[ -n "${seen[$key]:-}" ]] || fail "$key is missing from $ENV_FILE"
  done
  grep -q 'CHANGE_ME' "$ENV_FILE" && fail "$ENV_FILE contains a placeholder"

  local image
  image="$(read_env_value CARRYFORTH_RELAY_IMAGE)"
  validate_relay_image "$image"
}

require_env() {
  validate_env_file
}

compose() {
  docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" "$@"
}

random_hex() {
  openssl rand -hex "$1"
}

validate_relay_image() {
  local image="$1"
  local semver='(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?'
  [[ "$image" =~ ^ghcr\.io/lgyanami/carryforth-relay:${semver}(@sha256:[0-9a-f]{64})?$ ]] ||
    fail "image must be the canonical Carryforth Relay with an exact semver tag, optionally pinned to its sha256 digest"
}

relay_image_version() {
  local image="$1"
  local coordinate="${image#ghcr.io/lgyanami/carryforth-relay:}"
  printf '%s\n' "${coordinate%%@sha256:*}"
}

# Prints -1, 0, or 1 when the left semantic version is respectively lower,
# equal, or higher than the right one. The release coordinate validator has
# already rejected malformed core versions.
semver_compare() {
  local left="${1#v}"
  local right="${2#v}"
  local left_core="${left%%-*}"
  local right_core="${right%%-*}"
  local left_pre=""
  local right_pre=""
  [[ "$left" == *-* ]] && left_pre="${left#*-}"
  [[ "$right" == *-* ]] && right_pre="${right#*-}"

  local -a left_parts right_parts left_ids right_ids
  IFS='.' read -r -a left_parts <<< "$left_core"
  IFS='.' read -r -a right_parts <<< "$right_core"
  local index
  for index in 0 1 2; do
    if ((10#${left_parts[$index]} < 10#${right_parts[$index]})); then
      printf '%s\n' -1
      return
    fi
    if ((10#${left_parts[$index]} > 10#${right_parts[$index]})); then
      printf '%s\n' 1
      return
    fi
  done

  if [[ -z "$left_pre" && -z "$right_pre" ]]; then
    printf '%s\n' 0
    return
  fi
  if [[ -z "$left_pre" ]]; then
    printf '%s\n' 1
    return
  fi
  if [[ -z "$right_pre" ]]; then
    printf '%s\n' -1
    return
  fi

  IFS='.' read -r -a left_ids <<< "$left_pre"
  IFS='.' read -r -a right_ids <<< "$right_pre"
  local max_ids=${#left_ids[@]}
  (( ${#right_ids[@]} > max_ids )) && max_ids=${#right_ids[@]}
  local left_id right_id
  for ((index = 0; index < max_ids; index += 1)); do
    if (( index >= ${#left_ids[@]} )); then
      printf '%s\n' -1
      return
    fi
    if (( index >= ${#right_ids[@]} )); then
      printf '%s\n' 1
      return
    fi
    left_id="${left_ids[$index]}"
    right_id="${right_ids[$index]}"
    [[ "$left_id" == "$right_id" ]] && continue
    if [[ "$left_id" =~ ^[0-9]+$ && "$right_id" =~ ^[0-9]+$ ]]; then
      if ((10#$left_id < 10#$right_id)); then
        printf '%s\n' -1
      else
        printf '%s\n' 1
      fi
      return
    fi
    if [[ "$left_id" =~ ^[0-9]+$ ]]; then
      printf '%s\n' -1
      return
    fi
    if [[ "$right_id" =~ ^[0-9]+$ ]]; then
      printf '%s\n' 1
      return
    fi
    if [[ "$left_id" < "$right_id" ]]; then
      printf '%s\n' -1
    else
      printf '%s\n' 1
    fi
    return
  done
  printf '%s\n' 0
}

read_relay_image() {
  read_env_value CARRYFORTH_RELAY_IMAGE
}

write_relay_image() {
  local image="$1"
  local temp_file
  temp_file="$(mktemp "$SCRIPT_DIR/.env.tmp.XXXXXX")"
  trap 'rm -f -- "$temp_file"' EXIT
  trap 'rm -f -- "$temp_file"; exit 130' HUP INT TERM
  if ! awk -v image="$image" '
    BEGIN { found = 0 }
    $0 ~ /^CARRYFORTH_RELAY_IMAGE=/ {
      if (!found) {
        print "CARRYFORTH_RELAY_IMAGE=" image
        found = 1
      }
      next
    }
    { print }
    END { if (!found) exit 42 }
  ' "$ENV_FILE" >"$temp_file"; then
    rm -f "$temp_file"
    fail "CARRYFORTH_RELAY_IMAGE is missing from $ENV_FILE"
  fi
  chmod 600 "$temp_file"
  mv "$temp_file" "$ENV_FILE"
  temp_file=""
  trap - EXIT HUP INT TERM
  validate_env_file
}

parse_image_option() {
  local image=""
  shift
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --image)
        [[ $# -ge 2 ]] || fail "--image requires a value"
        image="$2"
        shift 2
        ;;
      *) fail "unknown image option: $1" ;;
    esac
  done
  [[ -n "$image" ]] || fail "--image <pinned-carryforth-relay-image> is required"
  validate_relay_image "$image"
  printf '%s\n' "$image"
}

init_env() {
  require_linux
  require_command openssl
  local image=""
  shift
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --image)
        [[ $# -ge 2 ]] || fail "--image requires a value"
        image="$2"
        shift 2
        ;;
      *) fail "unknown init option: $1" ;;
    esac
  done
  [[ -n "$image" ]] || fail "Usage: ./run.sh init --image <pinned-carryforth-relay-image>"
  validate_relay_image "$image"

  if [[ -e "$ENV_FILE" ]]; then
    fail "$ENV_FILE already exists; init is intentionally non-overwriting"
  fi

  umask 077
  local temp_file
  temp_file="$(mktemp "$SCRIPT_DIR/.env.tmp.XXXXXX")"
  trap 'rm -f -- "$temp_file"' EXIT
  trap 'rm -f -- "$temp_file"; exit 130' HUP INT TERM
  {
    printf 'CARRYFORTH_RELAY_IMAGE=%s\n' "$image"
    printf 'CARRYFORTH_POSTGRES_PORT=55432\n'
    printf 'CARRYFORTH_REDIS_PORT=56379\n'
    printf 'CARRYFORTH_MINIO_PORT=59000\n'
    printf 'CARRYFORTH_HEALTH_PORT=18080\n'
    printf 'CARRYFORTH_METRICS_PORT=19102\n'
    printf 'POSTGRES_DB=carryforth\n'
    printf 'POSTGRES_USER=carryforth\n'
    printf 'POSTGRES_PASSWORD=%s\n' "$(random_hex 24)"
    printf 'REDIS_PASSWORD=%s\n' "$(random_hex 24)"
    printf 'CARRYFORTH_S3_ACCESS_KEY=%s\n' "$(random_hex 16)"
    printf 'CARRYFORTH_S3_SECRET_KEY=%s\n' "$(random_hex 32)"
    printf 'CARRYFORTH_S3_BUCKET=carryforth-media\n'
    printf 'CARRYFORTH_RELAY_PRIVATE_KEY=%s\n' "$(random_hex 32)"
    printf 'CARRYFORTH_GIT_HOOK_HMAC_SECRET=%s\n' "$(random_hex 32)"
  } >"$temp_file"
  chmod 600 "$temp_file"
  if ! ln "$temp_file" "$ENV_FILE"; then
    fail "$ENV_FILE was created concurrently; refusing to overwrite it"
  fi
  rm -f "$temp_file"
  temp_file=""
  trap - EXIT HUP INT TERM
  log "created $ENV_FILE with stable local secrets"
  log "back up this file together with the named Docker volumes"
}

backup_hint() {
  cat <<'MSG'
Back up these items from the same maintenance window:

- deploy/local/.env
- carryforth-local_carryforth-postgres-data
- carryforth-local_carryforth-minio-data
- carryforth-local_carryforth-git-data
- carryforth-local_carryforth-redis-data

The stop, start, restart, pull and upgrade commands never delete these volumes.
MSG
}

case "${1:-help}" in
  init)
    init_env "$@"
    ;;
  start|up)
    require_linux
    require_env
    require_runtime
    compose up -d --wait
    log "Local Relay is ready at ws://localhost:3000"
    ;;
  stop)
    require_env
    require_runtime
    compose stop
    log "containers stopped; data volumes were retained"
    ;;
  restart)
    require_linux
    require_env
    require_runtime
    compose up -d --wait --force-recreate relay
    ;;
  pull)
    require_env
    require_runtime
    compose pull
    ;;
  upgrade)
    require_linux
    require_env
    require_runtime
    new_image="$(parse_image_option "$@")"
    old_image="$(read_relay_image)"
    [[ -n "$old_image" ]] || fail "CARRYFORTH_RELAY_IMAGE is missing from $ENV_FILE"
    if [[ "$new_image" == "$old_image" ]]; then
      fail "the requested Relay image is already pinned"
    fi
    old_version="$(relay_image_version "$old_image")"
    new_version="$(relay_image_version "$new_image")"
    version_order="$(semver_compare "$new_version" "$old_version")"
    if [[ "$version_order" == "-1" ]]; then
      fail "Relay downgrade from $old_version to $new_version is forbidden"
    fi
    if [[ "$version_order" == "0" &&
      ( "$old_image" == *@sha256:* || "$new_image" != *@sha256:* ) ]]; then
      fail "same-version replacement is allowed only when strengthening an unpinned tag to its release-qualified digest"
    fi
    backup_hint
    write_relay_image "$new_image"
    if ! compose pull relay; then
      write_relay_image "$old_image"
      log "new Relay image pull failed before any new container started; restoring the previous image pin"
      if compose up -d --wait relay; then
        fail "upgrade pull failed; the previous Relay service is running at $old_image; no volume was deleted"
      fi
      fail "upgrade pull failed and the previous Relay service could not be restored automatically; the image pin is $old_image and no volume was deleted"
    fi
    if ! compose up -d --wait relay; then
      # The new binary may already have applied a forward-only migration. Do
      # not start the old binary against an unknown newer schema.
      compose stop relay >/dev/null 2>&1 || true
      fail "new Relay failed after startup began; it was stopped and the pin remains $new_image. No volume was deleted. Do not start the old image until migration compatibility or a backup restore has been verified"
    fi
    log "Relay upgraded from $old_image to $new_image; data volumes were retained"
    ;;
  status|ps)
    require_env
    require_runtime
    compose ps
    ;;
  logs)
    require_env
    require_runtime
    shift || true
    [[ $# -gt 0 ]] || set -- relay
    compose logs -f "$@"
    ;;
  config)
    require_env
    require_runtime
    compose config --quiet
    log "effective configuration is valid; secret values are intentionally not rendered"
    docker compose --env-file "$ENV_FILE" -f "$COMPOSE_FILE" config --no-interpolate
    ;;
  validate)
    require_linux
    require_env
    log "local configuration is valid"
    ;;
  backup-hint)
    backup_hint
    ;;
  help|-h|--help)
    cat <<'MSG'
Usage: ./run.sh <command>

Commands:
  init --image <image>  Create a non-overwriting .env with stable secrets
  start                 Start the local stack and wait for readiness
  stop                  Stop containers without deleting data or containers
  restart               Recreate only the Relay container
  pull                   Pull the pinned images
  upgrade --image <image> Print backup scope, pull a new pinned Relay, and restart
  status                 Show container status
  logs [service]         Follow logs (default: relay)
  config                 Render the effective Compose configuration
  validate               Validate .env without contacting Docker
  backup-hint            Print the data backup scope

There is intentionally no reset, down -v, remote Relay, Push, TLS or hosted
Community switch in this local-only entrypoint.
MSG
    ;;
  *) fail "unknown command: $1" ;;
esac
