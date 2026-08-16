#!/usr/bin/env bash
# Verify first-start configuration without starting Docker, Relay, or Desktop.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEMP_ROOT="$(mktemp -d)"
cleanup() {
  local status=$?
  find "${TEMP_ROOT}" -depth -delete
  exit "${status}"
}
trap cleanup EXIT

unset_semantic_env=(
  -u BUZZ_SEMANTIC_WORKER_ENABLED
  -u BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE
  -u BUZZ_SEMANTIC_API_KEY
  -u BUZZ_SEMANTIC_BASE_URL
  -u BUZZ_SEMANTIC_REQUEST_MODEL
  -u LLM_API_KEY
  -u LLM_BASE_URL
  -u LLM_MODEL
  -u BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY
  -u BUZZ_RELAY_PRIVATE_KEY
)

missing_env="${TEMP_ROOT}/missing.env"
missing_log="${TEMP_ROOT}/missing.log"
if env "${unset_semantic_env[@]}" \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${missing_env}" \
  </dev/null >"${missing_log}" 2>&1; then
  echo "semantic configuration accepted a missing key without a terminal" >&2
  exit 1
fi
rg --quiet 'required semantic Provider values are missing' "${missing_log}"
rg --quiet 'BUZZ_SEMANTIC_API_KEY' "${missing_log}"
rg --quiet 'BUZZ_SEMANTIC_BASE_URL' "${missing_log}"
rg --quiet 'BUZZ_SEMANTIC_REQUEST_MODEL' "${missing_log}"
[[ "$(stat -c '%a' "${missing_env}")" == "600" ]]

printf '\nBUZZ_SEMANTIC_API_KEY=synthetic-provider-key\n' >>"${missing_env}"
printf 'BUZZ_SEMANTIC_BASE_URL=https://provider.invalid/v1/\n' >>"${missing_env}"
printf 'BUZZ_SEMANTIC_REQUEST_MODEL=synthetic-embedding-model\n' >>"${missing_env}"
configured_log="${TEMP_ROOT}/configured.log"
env "${unset_semantic_env[@]}" \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${missing_env}" \
  </dev/null >"${configured_log}" 2>&1
! rg --fixed-strings --quiet 'synthetic-provider-key' "${configured_log}"

# shellcheck disable=SC1090
source "${missing_env}"
[[ "${BUZZ_BIND_ADDR}" == "127.0.0.1:3000" ]]
[[ "${BUZZ_SEMANTIC_WORKER_ENABLED}" == "true" ]]
[[ "${BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE}" == "true" ]]
[[ "${BUZZ_SEMANTIC_API_KEY}" == "synthetic-provider-key" ]]
[[ "${BUZZ_SEMANTIC_BASE_URL}" == "https://provider.invalid/v1/" ]]
[[ "${BUZZ_SEMANTIC_REQUEST_MODEL}" == "synthetic-embedding-model" ]]
[[ "${BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY}" == "trusted-single-relay" ]]
[[ "${BUZZ_RELAY_PRIVATE_KEY}" =~ ^[0-9a-f]{64}$ ]]
[[ "$(stat -c '%a' "${missing_env}")" == "600" ]]

first_relay_key="${BUZZ_RELAY_PRIVATE_KEY}"
first_hash="$(sha256sum "${missing_env}" | awk '{print $1}')"
env "${unset_semantic_env[@]}" \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${missing_env}" \
  </dev/null >/dev/null
# shellcheck disable=SC1090
source "${missing_env}"
[[ "${BUZZ_RELAY_PRIVATE_KEY}" == "${first_relay_key}" ]]
[[ "$(sha256sum "${missing_env}" | awk '{print $1}')" == "${first_hash}" ]]
[[ "$(rg -c '^BUZZ_SEMANTIC_API_KEY=' "${missing_env}")" == "1" ]]
[[ "$(rg -c '^BUZZ_RELAY_PRIVATE_KEY=' "${missing_env}")" == "1" ]]

llm_env="${TEMP_ROOT}/llm.env"
node "${SCRIPT_DIR}/update-local-env.mjs" \
  --target "${llm_env}" \
  --source-template "${REPO_ROOT}/.env.example"
printf '\nLLM_API_KEY=synthetic-shared-key\n' >>"${llm_env}"
printf 'LLM_BASE_URL=https://shared-provider.invalid/v1/\n' >>"${llm_env}"
printf 'LLM_MODEL=synthetic-shared-model\n' >>"${llm_env}"
env "${unset_semantic_env[@]}" \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${llm_env}" \
  </dev/null >"${TEMP_ROOT}/llm.log" 2>&1
! rg --fixed-strings --quiet 'synthetic-shared-key' "${TEMP_ROOT}/llm.log"
rg --quiet '^LLM_API_KEY="synthetic-shared-key"$' "${llm_env}"
rg --quiet '^LLM_BASE_URL="https://shared-provider.invalid/v1/"$' "${llm_env}"
rg --quiet '^LLM_MODEL="synthetic-shared-model"$' "${llm_env}"
! rg --quiet '^BUZZ_SEMANTIC_API_KEY=' "${llm_env}"

legacy_bind_env="${TEMP_ROOT}/legacy-bind.env"
cp "${REPO_ROOT}/.env.example" "${legacy_bind_env}"
sed -i 's/^BUZZ_BIND_ADDR=127\.0\.0\.1:3000$/BUZZ_BIND_ADDR=0.0.0.0:3000/' \
  "${legacy_bind_env}"
node "${SCRIPT_DIR}/update-local-env.mjs" \
  --target "${legacy_bind_env}" \
  --source-template "${REPO_ROOT}/.env.example"
[[ "$(rg -c '^BUZZ_BIND_ADDR=127\.0\.0\.1:3000$' "${legacy_bind_env}")" == "1" ]]
! rg --quiet '^BUZZ_BIND_ADDR=0\.0\.0\.0:3000$' "${legacy_bind_env}"

custom_bind_env="${TEMP_ROOT}/custom-bind.env"
cp "${REPO_ROOT}/.env.example" "${custom_bind_env}"
sed -i 's/^BUZZ_BIND_ADDR=127\.0\.0\.1:3000$/BUZZ_BIND_ADDR=192.0.2.10:3030/' \
  "${custom_bind_env}"
node "${SCRIPT_DIR}/update-local-env.mjs" \
  --target "${custom_bind_env}" \
  --source-template "${REPO_ROOT}/.env.example"
rg --quiet '^BUZZ_BIND_ADDR=192\.0\.2\.10:3030$' "${custom_bind_env}"

compose_json="$(docker compose \
  -f "${REPO_ROOT}/docker-compose.yml" config --format json)"
for service in postgres redis adminer keycloak minio prometheus; do
  node -e '
    const model = JSON.parse(process.argv[1]);
    const service = model.services[process.argv[2]];
    if (!service) process.exit(1);
    if (!/@sha256:[a-f0-9]{64}$/.test(service.image ?? "")) process.exit(1);
    for (const port of service.ports ?? []) {
      if (port.host_ip !== "127.0.0.1") process.exit(1);
    }
  ' "${compose_json}" "${service}"
done

python3 - "${REPO_ROOT}/scripts/dev-start.sh" <<'PY'
from pathlib import Path
import sys

text = Path(sys.argv[1]).read_text(encoding="utf-8")
curl_preflight = text.index("command -v curl")
semantic_config = text.index('"${SCRIPT_DIR}/configure-local-semantic.sh"')
compose_start = text.index("docker compose up -d")
managed_launch = text.index("nohup python3")
assert curl_preflight < semantic_config < compose_start < managed_launch
PY

disabled_env="${TEMP_ROOT}/disabled.env"
node "${SCRIPT_DIR}/update-local-env.mjs" \
  --target "${disabled_env}" \
  --source-template "${REPO_ROOT}/.env.example"
printf '\nBUZZ_SEMANTIC_WORKER_ENABLED=false\n' >>"${disabled_env}"
printf 'BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=false\n' >>"${disabled_env}"
env "${unset_semantic_env[@]}" \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${disabled_env}" \
  </dev/null >/dev/null
# shellcheck disable=SC1090
source "${disabled_env}"
[[ "${BUZZ_SEMANTIC_WORKER_ENABLED}" == "false" ]]
[[ "${BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE}" == "false" ]]
! rg --quiet '^BUZZ_SEMANTIC_API_KEY=' "${disabled_env}"
! rg --quiet '^BUZZ_RELAY_PRIVATE_KEY=' "${disabled_env}"

special_env="${TEMP_ROOT}/special.env"
special_key='synthetic-$-`-"-\\-provider-key'
BUZZ_SEMANTIC_API_KEY="${special_key}" \
  BUZZ_SEMANTIC_BASE_URL=https://special-provider.invalid/v1/ \
  BUZZ_SEMANTIC_REQUEST_MODEL=synthetic-special-model \
  BUZZ_SEMANTIC_WORKER_ENABLED=true \
  BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${special_env}" \
  </dev/null >"${TEMP_ROOT}/special.log" 2>&1
! rg --fixed-strings --quiet "${special_key}" "${TEMP_ROOT}/special.log"
unset BUZZ_SEMANTIC_API_KEY BUZZ_SEMANTIC_WORKER_ENABLED
unset BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE BUZZ_RELAY_PRIVATE_KEY
# shellcheck disable=SC1090
source "${special_env}"
[[ "${BUZZ_SEMANTIC_API_KEY}" == "${special_key}" ]]

xtrace_env="${TEMP_ROOT}/xtrace.env"
xtrace_log="${TEMP_ROOT}/xtrace.log"
BUZZ_SEMANTIC_API_KEY=synthetic-xtrace-secret \
  BUZZ_SEMANTIC_BASE_URL=https://xtrace-provider.invalid/v1/ \
  BUZZ_SEMANTIC_REQUEST_MODEL=synthetic-xtrace-model \
  BUZZ_SEMANTIC_WORKER_ENABLED=true \
  BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true \
  bash -x "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${xtrace_env}" \
  </dev/null >"${xtrace_log}" 2>&1
! rg --fixed-strings --quiet 'synthetic-xtrace-secret' "${xtrace_log}"
rg --quiet 'Shell tracing disabled' "${xtrace_log}"

symlink_target="${TEMP_ROOT}/symlink-target"
symlink_env="${TEMP_ROOT}/symlink.env"
printf 'unchanged\n' >"${symlink_target}"
ln -s "${symlink_target}" "${symlink_env}"
if env "${unset_semantic_env[@]}" \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${symlink_env}" \
  </dev/null >/dev/null 2>&1; then
  echo "semantic configuration accepted a symlink environment target" >&2
  exit 1
fi
[[ "$(<"${symlink_target}")" == "unchanged" ]]

invalid_env="${TEMP_ROOT}/invalid.env"
node "${SCRIPT_DIR}/update-local-env.mjs" \
  --target "${invalid_env}" \
  --source-template "${REPO_ROOT}/.env.example"
printf '\nBUZZ_SEMANTIC_WORKER_ENABLED=sometimes\n' >>"${invalid_env}"
if env "${unset_semantic_env[@]}" \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${invalid_env}" \
  </dev/null >"${TEMP_ROOT}/invalid.log" 2>&1; then
  echo "semantic configuration accepted an invalid process switch" >&2
  exit 1
fi
rg --quiet 'BUZZ_SEMANTIC_WORKER_ENABLED must be true or false' \
  "${TEMP_ROOT}/invalid.log"

partial_env="${TEMP_ROOT}/partial.env"
node "${SCRIPT_DIR}/update-local-env.mjs" \
  --target "${partial_env}" \
  --source-template "${REPO_ROOT}/.env.example"
printf '\nBUZZ_SEMANTIC_API_KEY=synthetic-partial-key\n' >>"${partial_env}"
if env "${unset_semantic_env[@]}" \
  "${SCRIPT_DIR}/configure-local-semantic.sh" --env-file "${partial_env}" \
  </dev/null >"${TEMP_ROOT}/partial.log" 2>&1; then
  echo "semantic configuration accepted a partial Provider configuration" >&2
  exit 1
fi
rg --quiet 'BUZZ_SEMANTIC_BASE_URL' "${TEMP_ROOT}/partial.log"
rg --quiet 'BUZZ_SEMANTIC_REQUEST_MODEL' "${TEMP_ROOT}/partial.log"
! rg --quiet 'missing:.*BUZZ_SEMANTIC_API_KEY' "${TEMP_ROOT}/partial.log"

python3 - "${REPO_ROOT}/start.sh" <<'PY'
from pathlib import Path
import sys

text = Path(sys.argv[1]).read_text(encoding="utf-8")
assert 'exec "${REPO_ROOT}/scripts/dev-start.sh" "$@"' in text
PY

rg --quiet '^"\$\{SCRIPT_DIR\}/configure-local-semantic\.sh"$' \
  "${REPO_ROOT}/scripts/dev-rebuild-start.sh"
rg --quiet '^_configure-local-semantic: bootstrap$' "${REPO_ROOT}/Justfile"
rg --quiet '^\s*\./scripts/configure-local-semantic\.sh$' "${REPO_ROOT}/Justfile"
grep -Fqx '.env' "${REPO_ROOT}/.gitignore"
! rg --ignore-case --quiet \
  '(apt(-get)?|brew|dnf|yum|pacman|snap)[[:space:]]+.*install' \
  "${REPO_ROOT}/start.sh" \
  "${REPO_ROOT}/scripts/dev-start.sh" \
  "${REPO_ROOT}/scripts/configure-local-semantic.sh"
! rg --quiet 'ark\.cn-beijing|doubao-embedding-vision' \
  "${REPO_ROOT}/scripts/configure-local-semantic.sh" \
  "${REPO_ROOT}/.env.example" \
  "${REPO_ROOT}/README.md" \
  "${REPO_ROOT}/CONTRIBUTING.md"

printf 'source dev-start configuration tests passed\n'
