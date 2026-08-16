#!/usr/bin/env bash
# Idempotently prepare/finalize semantic state for the exact local Community.
set -euo pipefail

# .env contains Provider credentials; never copy them into an xtrace log.
if [[ "$-" == *x* ]]; then
  set +x
  printf '[semantic-bootstrap] Shell tracing disabled while loading local credentials.\n' >&2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PHASE="${1:-}"

case "${PHASE}" in
  prepare|finalize) ;;
  *)
    printf 'Usage: bootstrap-local-semantic.sh prepare|finalize\n' >&2
    exit 2
    ;;
esac

cd "${REPO_ROOT}"
# shellcheck disable=SC1091
source "${REPO_ROOT}/bin/activate-hermit"
set -o allexport
# shellcheck disable=SC1091
source "${REPO_ROOT}/.env"
set +o allexport

switches=(
  BUZZ_SEMANTIC_WORKER_ENABLED
  BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE
  CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE
  CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE
)
enabled=0
disabled=0
for name in "${switches[@]}"; do
  value="${!name-}"
  case "${value,,}" in
    true) enabled=$((enabled + 1)) ;;
    false) disabled=$((disabled + 1)) ;;
    *)
      printf '[semantic-bootstrap] ERROR: %s must be true or false\n' "${name}" >&2
      exit 1
      ;;
  esac
done

if ((disabled == ${#switches[@]})); then
  printf '[semantic-bootstrap] All semantic process switches are explicitly disabled; skipping %s.\n' \
    "${PHASE}"
  exit 0
fi
if ((enabled != ${#switches[@]})); then
  printf '[semantic-bootstrap] Partial semantic process configuration detected; automatic full-capability bootstrap is skipped.\n'
  exit 0
fi

timeout_seconds="${BUZZ_LOCAL_SEMANTIC_BOOTSTRAP_TIMEOUT_SECONDS:-600}"
if [[ ! "${timeout_seconds}" =~ ^[1-9][0-9]*$ ]] || ((timeout_seconds > 3600)); then
  printf '[semantic-bootstrap] ERROR: BUZZ_LOCAL_SEMANTIC_BOOTSTRAP_TIMEOUT_SECONDS must be in 1..=3600\n' >&2
  exit 1
fi

arguments=(
  semantic local-bootstrap
  --phase "${PHASE}"
  --acknowledge-local-provider-egress
  --wait-seconds "${timeout_seconds}"
)
if [[ "${PHASE}" == prepare ]]; then
  cargo run -p buzz-admin -- "${arguments[@]}"
else
  [[ -x ./target/debug/buzz-admin ]] || {
    printf '[semantic-bootstrap] ERROR: prepare phase did not build target/debug/buzz-admin\n' >&2
    exit 1
  }
  ./target/debug/buzz-admin "${arguments[@]}"
fi
