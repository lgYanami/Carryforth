#!/usr/bin/env bash
# Prepare the ignored local .env used by supported source-development launchers.
# System prerequisites are checked elsewhere; this script never installs tools.
set -euo pipefail

# Never expose Provider or Relay keys when a caller enables shell tracing.
if [[ "$-" == *x* ]]; then
  set +x
  printf '[semantic-config] Shell tracing disabled while handling local credentials.\n' >&2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ENV_FILE="${REPO_ROOT}/.env"
TEMPLATE_FILE="${REPO_ROOT}/.env.example"

fail() {
  printf '[semantic-config] ERROR: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage: configure-local-semantic.sh [--env-file PATH]

Initializes the ignored local environment and configures the semantic Provider
for source-development startup. Missing Provider values are requested on a
terminal; only the API key uses hidden input. This script does not install
system software.
EOF
}

while (($# > 0)); do
  case "$1" in
    --env-file)
      (($# >= 2)) || fail "--env-file requires a path"
      ENV_FILE="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

command -v node >/dev/null 2>&1 || fail "Hermit Node is unavailable; activate ./bin/activate-hermit"
if [[ "${ENV_FILE}" == "${REPO_ROOT}/.env" ]] &&
  ! grep -Fqx '.env' "${REPO_ROOT}/.gitignore"; then
  fail "refusing to store local credentials because .env is not ignored"
fi

env_file_existed=false
[[ -e "${ENV_FILE}" || -L "${ENV_FILE}" ]] && env_file_existed=true
node "${SCRIPT_DIR}/update-local-env.mjs" \
  --target "${ENV_FILE}" \
  --source-template "${TEMPLATE_FILE}"
if [[ "${env_file_existed}" == false ]]; then
  printf '[semantic-config] Created private local environment: %s\n' "${ENV_FILE}"
fi

env_file_value() {
  local name="$1"
  bash -c '
    set -euo pipefail
    set -o allexport
    # shellcheck disable=SC1090
    source "$1"
    set +o allexport
    name="$2"
    printf "%s" "${!name-}"
  ' _ "${ENV_FILE}" "${name}"
}

effective_value() {
  local name="$1"
  if declare -p "${name}" >/dev/null 2>&1; then
    printf '%s' "${!name-}"
  else
    env_file_value "${name}"
  fi
}

normalize_boolean() {
  local name="$1"
  local value="$2"
  case "${value}" in
    [Tt][Rr][Uu][Ee])
      printf 'true'
      ;;
    [Ff][Aa][Ll][Ss][Ee])
      printf 'false'
      ;;
    *)
      fail "${name} must be true or false"
      ;;
  esac
}

worker_enabled="$(effective_value BUZZ_SEMANTIC_WORKER_ENABLED)"
query_http_available="$(effective_value BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE)"
coordinate_search_http_available="$(effective_value \
  CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE)"
one_hop_search_http_available="$(effective_value \
  CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE)"
semantic_base_url="$(effective_value BUZZ_SEMANTIC_BASE_URL)"
semantic_request_model="$(effective_value BUZZ_SEMANTIC_REQUEST_MODEL)"
fleet_policy="$(effective_value BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY)"
semantic_api_key="$(effective_value BUZZ_SEMANTIC_API_KEY)"
relay_private_key="$(effective_value BUZZ_RELAY_PRIVATE_KEY)"
provider_env_family=compatibility
provider_api_key_name=BUZZ_SEMANTIC_API_KEY
provider_base_url_name=BUZZ_SEMANTIC_BASE_URL
provider_model_name=BUZZ_SEMANTIC_REQUEST_MODEL
if [[ ! "${semantic_api_key}${semantic_base_url}${semantic_request_model}" =~ [^[:space:]] ]]; then
  llm_api_key="$(effective_value LLM_API_KEY)"
  llm_base_url="$(effective_value LLM_BASE_URL)"
  llm_model="$(effective_value LLM_MODEL)"
  if [[ "${llm_api_key}${llm_base_url}${llm_model}" =~ [^[:space:]] ]]; then
    provider_env_family=llm
    provider_api_key_name=LLM_API_KEY
    provider_base_url_name=LLM_BASE_URL
    provider_model_name=LLM_MODEL
    semantic_api_key="${llm_api_key}"
    semantic_base_url="${llm_base_url}"
    semantic_request_model="${llm_model}"
  fi
fi

worker_enabled="$(normalize_boolean BUZZ_SEMANTIC_WORKER_ENABLED "${worker_enabled:-true}")"
query_http_available="$(normalize_boolean \
  BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE \
  "${query_http_available:-true}")"
coordinate_search_http_available="$(normalize_boolean \
  CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE \
  "${coordinate_search_http_available:-true}")"
one_hop_search_http_available="$(normalize_boolean \
  CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE \
  "${one_hop_search_http_available:-true}")"
fleet_policy="${fleet_policy:-trusted-single-relay}"

if [[ "${worker_enabled}" == true ||
      "${query_http_available}" == true ||
      "${coordinate_search_http_available}" == true ||
      "${one_hop_search_http_available}" == true ]]; then
  missing_provider_values=()
  [[ "${semantic_api_key}" =~ [^[:space:]] ]] ||
    missing_provider_values+=("${provider_api_key_name}")
  [[ "${semantic_base_url}" =~ [^[:space:]] ]] ||
    missing_provider_values+=("${provider_base_url_name}")
  [[ "${semantic_request_model}" =~ [^[:space:]] ]] ||
    missing_provider_values+=("${provider_model_name}")

  if ((${#missing_provider_values[@]} > 0)); then
    if [[ ! -t 0 ]]; then
      fail "required semantic Provider values are missing: ${missing_provider_values[*]}; rerun in a terminal, set them in ${ENV_FILE}, or explicitly disable all four semantic process switches"
    fi
    printf 'Semantic Provider configuration is required for local semantic features.\n' >&2
    printf 'Values will be stored only in the Git-ignored %s (mode 0600).\n' \
      "${ENV_FILE}" >&2
  fi

  if [[ ! "${semantic_api_key}" =~ [^[:space:]] ]]; then
    read -r -s -p 'Provider API key: ' semantic_api_key
    printf '\n' >&2
    [[ "${semantic_api_key}" =~ [^[:space:]] ]] || fail "Provider API key cannot be empty"
  fi
  if [[ ! "${semantic_base_url}" =~ [^[:space:]] ]]; then
    read -r -p 'Provider base URL: ' semantic_base_url
    [[ "${semantic_base_url}" =~ [^[:space:]] ]] || fail "Provider base URL cannot be empty"
  fi
  if [[ ! "${semantic_request_model}" =~ [^[:space:]] ]]; then
    read -r -p 'Provider request model: ' semantic_request_model
    [[ "${semantic_request_model}" =~ [^[:space:]] ]] || fail "Provider request model cannot be empty"
  fi
fi

if [[ ("${query_http_available}" == true ||
       "${coordinate_search_http_available}" == true ||
       "${one_hop_search_http_available}" == true) &&
      -z "${relay_private_key}" ]]; then
  relay_private_key="$(node -e \
    'process.stdout.write(require("node:crypto").randomBytes(32).toString("hex"))')"
  [[ "${relay_private_key}" =~ ^[0-9a-f]{64}$ ]] || fail "failed to generate local Relay key"
  printf '[semantic-config] Generated a stable local Relay signing key.\n'
fi

export CARRYFORTH_LOCAL_WORKER_ENABLED="${worker_enabled}"
export CARRYFORTH_LOCAL_QUERY_HTTP_AVAILABLE="${query_http_available}"
export CARRYFORTH_LOCAL_COORDINATE_SEARCH_HTTP_AVAILABLE="${coordinate_search_http_available}"
export CARRYFORTH_LOCAL_ONE_HOP_SEARCH_HTTP_AVAILABLE="${one_hop_search_http_available}"
export CARRYFORTH_LOCAL_FLEET_POLICY="${fleet_policy}"
if [[ "${provider_env_family}" == llm ]]; then
  if [[ -n "${semantic_api_key}" ]]; then
    export CARRYFORTH_LOCAL_LLM_API_KEY="${semantic_api_key}"
  fi
  if [[ -n "${semantic_base_url}" ]]; then
    export CARRYFORTH_LOCAL_LLM_BASE_URL="${semantic_base_url}"
  fi
  if [[ -n "${semantic_request_model}" ]]; then
    export CARRYFORTH_LOCAL_LLM_MODEL="${semantic_request_model}"
  fi
else
  if [[ -n "${semantic_api_key}" ]]; then
    export CARRYFORTH_LOCAL_SEMANTIC_API_KEY="${semantic_api_key}"
  fi
  if [[ -n "${semantic_base_url}" ]]; then
    export CARRYFORTH_LOCAL_SEMANTIC_BASE_URL="${semantic_base_url}"
  fi
  if [[ -n "${semantic_request_model}" ]]; then
    export CARRYFORTH_LOCAL_SEMANTIC_REQUEST_MODEL="${semantic_request_model}"
  fi
fi
if [[ -n "${relay_private_key}" ]]; then
  export CARRYFORTH_LOCAL_RELAY_PRIVATE_KEY="${relay_private_key}"
fi

node "${SCRIPT_DIR}/update-local-env.mjs" \
  --target "${ENV_FILE}" \
  --source-template "${TEMPLATE_FILE}"

provider_status="not-required"
if [[ -n "${semantic_api_key}" ]]; then
  provider_status="configured"
fi
printf '[semantic-config] Semantic Worker=%s, graph query=%s, coordinate search=%s, one-hop search=%s, Provider credentials=%s\n' \
  "${worker_enabled}" \
  "${query_http_available}" \
  "${coordinate_search_http_available}" \
  "${one_hop_search_http_available}" \
  "${provider_status}"
