#!/usr/bin/env bash
set -euo pipefail

# Contract marker consumed by the public release-surface gate. Do not turn this
# source-history tombstone back into an operational deployment wrapper.
readonly COMPOSE_ENTRYPOINT_RETIRED=1
readonly RETIRED_EXIT_CODE=78

retirement_notice() {
  cat <<'MSG'
deploy/compose is retired and cannot operate containers.

Use the supported local-only Carryforth stack instead:
  cd deploy/local
  ./run.sh init --image "$(cat RELAY_IMAGE)"
  ./run.sh start

No containers, volumes, databases, or application data were changed.
MSG
}

case "${1:-help}" in
  help|-h|--help)
    retirement_notice
    ;;
  *)
    retirement_notice >&2
    exit "${RETIRED_EXIT_CODE}"
    ;;
esac
