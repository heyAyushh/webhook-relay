#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_CONFIG_PATH="firecracker/firecracker-config.template.json"

CONFIG_PATH="${DEFAULT_CONFIG_PATH}"
SOCKET_PATH=""

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

usage() {
  cat <<'EOF_USAGE' >&2
Usage: scripts/run-firecracker.sh [options]

Options:
  --config <path>  Firecracker config JSON path (default: firecracker/firecracker-config.template.json)
  --socket <path>  API socket path (optional)
EOF_USAGE
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --config)
        CONFIG_PATH="$2"
        shift 2
        ;;
      --socket)
        SOCKET_PATH="$2"
        shift 2
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        usage
        die "unknown option: $1"
        ;;
    esac
  done
}

main() {
  parse_args "$@"

  command -v firecracker >/dev/null 2>&1 || die "firecracker binary not found"
  [ -f "${CONFIG_PATH}" ] || die "config file not found: ${CONFIG_PATH}"

  if [ ! -e /dev/kvm ]; then
    die "/dev/kvm not found; host does not support KVM for Firecracker"
  fi

  if [ -n "${SOCKET_PATH}" ]; then
    log "starting firecracker with config ${CONFIG_PATH} and socket ${SOCKET_PATH}"
    exec firecracker --config-file "${CONFIG_PATH}" --api-sock "${SOCKET_PATH}"
  fi

  log "starting firecracker with config ${CONFIG_PATH}"
  exec firecracker --config-file "${CONFIG_PATH}"
}

main "$@"
