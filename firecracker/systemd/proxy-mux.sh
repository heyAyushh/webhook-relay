#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

readonly DEFAULT_BROKER_INVENTORY_SCRIPT="${SCRIPT_DIR}/../runtime/broker_inventory.sh"
readonly DEFAULT_RELAY_PROXY_PORT="9445"
readonly DEFAULT_RELAY_VM_IP="172.30.0.20"
readonly DEFAULT_RELAY_VM_PORT="8080"
readonly DEFAULT_BROKER_PROXY_BIND_HOST="127.0.0.1"
readonly DEFAULT_BROKER_TARGET_PORT="9092"
readonly DEFAULT_ENABLE_RELAY_PROXY="false"
readonly DEFAULT_ENABLE_BROKER_PROXIES="false"

BROKER_INVENTORY_SCRIPT="${FIRECRACKER_BROKER_INVENTORY_SCRIPT:-${DEFAULT_BROKER_INVENTORY_SCRIPT}}"
RELAY_PROXY_PORT="${FIRECRACKER_RELAY_PROXY_PORT:-${DEFAULT_RELAY_PROXY_PORT}}"
RELAY_VM_IP="${FIRECRACKER_RELAY_VM_IP:-${DEFAULT_RELAY_VM_IP}}"
RELAY_VM_PORT="${FIRECRACKER_RELAY_VM_PORT:-${DEFAULT_RELAY_VM_PORT}}"
BROKER_PROXY_BIND_HOST="${FIRECRACKER_BROKER_PROXY_BIND_HOST:-${DEFAULT_BROKER_PROXY_BIND_HOST}}"
BROKER_TARGET_PORT="${FIRECRACKER_BROKER_TARGET_PORT:-${DEFAULT_BROKER_TARGET_PORT}}"
ENABLE_RELAY_PROXY="${FIRECRACKER_ENABLE_RELAY_PROXY:-${DEFAULT_ENABLE_RELAY_PROXY}}"
ENABLE_BROKER_PROXIES="${FIRECRACKER_ENABLE_BROKER_PROXIES:-${DEFAULT_ENABLE_BROKER_PROXIES}}"

declare -a PROXY_PIDS=()

require_cmd() {
  local command_name="$1"
  command -v "${command_name}" >/dev/null 2>&1 || {
    printf 'error: missing command: %s\n' "${command_name}" >&2
    exit 1
  }
}

log() {
  printf '%s\n' "$*" >&2
}

as_bool() {
  case "${1,,}" in
    1|true|yes|on) return 0 ;;
    *) return 1 ;;
  esac
}

cleanup() {
  local process_id=""
  for process_id in "${PROXY_PIDS[@]:-}"; do
    kill "${process_id}" 2>/dev/null || true
  done
  wait "${PROXY_PIDS[@]:-}" 2>/dev/null || true
}

start_proxy() {
  local listen_address="$1"
  local target_address="$2"

  socat "${listen_address}" "${target_address}" &
  PROXY_PIDS+=("$!")
  log "proxy-mux: ${listen_address} -> ${target_address}"
}

main() {
  trap cleanup EXIT INT TERM

  if ! as_bool "${ENABLE_RELAY_PROXY}" && ! as_bool "${ENABLE_BROKER_PROXIES}"; then
    log "proxy-mux: disabled (set FIRECRACKER_ENABLE_RELAY_PROXY and/or FIRECRACKER_ENABLE_BROKER_PROXIES to true)"
    exit 0
  fi

  require_cmd socat

  if as_bool "${ENABLE_RELAY_PROXY}"; then
    start_proxy \
      "TCP-LISTEN:${RELAY_PROXY_PORT},fork,reuseaddr" \
      "TCP:${RELAY_VM_IP}:${RELAY_VM_PORT}"
  fi

  if as_bool "${ENABLE_BROKER_PROXIES}"; then
    if [ -f "${BROKER_INVENTORY_SCRIPT}" ]; then
      # shellcheck disable=SC1090
      . "${BROKER_INVENTORY_SCRIPT}"
    fi

    if declare -F inventory_rows >/dev/null 2>&1; then
      mapfile -t broker_rows < <(inventory_rows)
    else
      broker_rows=()
    fi

    for broker_row in "${broker_rows[@]}"; do
      IFS=$'\t' read -r broker_id _ broker_ip _ _ host_proxy_port _ _ <<< "${broker_row}"
      if [[ "${host_proxy_port}" =~ ^[0-9]+$ ]] && [ "${host_proxy_port}" -gt 0 ]; then
        start_proxy \
          "TCP-LISTEN:${host_proxy_port},fork,reuseaddr,bind=${BROKER_PROXY_BIND_HOST}" \
          "TCP:${broker_ip}:${BROKER_TARGET_PORT}"
      else
        log "proxy-mux: skip broker ${broker_id} (no host proxy port set)"
      fi
    done
  fi

  if [ "${#PROXY_PIDS[@]}" -eq 0 ]; then
    log "proxy-mux: no proxies started"
    exit 0
  fi

  wait -n "${PROXY_PIDS[@]}"
  exit 1
}

main "$@"
