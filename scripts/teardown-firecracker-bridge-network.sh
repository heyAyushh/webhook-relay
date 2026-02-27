#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly DEFAULT_BRIDGE_NAME="fcbr0"
readonly DEFAULT_GUEST_CIDR="172.30.0.0/24"
readonly DEFAULT_TAP_LIST="tap-relay,tap-kafka"
readonly DEFAULT_BROKER_INVENTORY_SCRIPT="${REPO_ROOT}/firecracker/runtime/broker_inventory.sh"

BRIDGE_NAME="${FIRECRACKER_BRIDGE_NAME:-${DEFAULT_BRIDGE_NAME}}"
GUEST_CIDR="${FIRECRACKER_GUEST_CIDR:-${DEFAULT_GUEST_CIDR}}"
TAP_LIST="${FIRECRACKER_TAP_LIST:-${DEFAULT_TAP_LIST}}"
UPLINK_IFACE="${FIRECRACKER_UPLINK_IFACE:-}"
BROKER_INVENTORY_SCRIPT="${FIRECRACKER_BROKER_INVENTORY_SCRIPT:-${DEFAULT_BROKER_INVENTORY_SCRIPT}}"

log() {
  printf '%s\n' "$*" >&2
}

require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    printf 'error: run as root\n' >&2
    exit 1
  fi
}

auto_detect_uplink() {
  if [ -n "${UPLINK_IFACE}" ]; then
    return
  fi

  UPLINK_IFACE="$(ip route show default 2>/dev/null | awk '/default/ {print $5; exit}')"
}

collect_taps() {
  local tap_name=""
  local tap_csv_item=""

  declare -A tap_seen=()
  declare -a taps=()

  IFS=',' read -r -a tap_csv_items <<< "${TAP_LIST}"
  for tap_csv_item in "${tap_csv_items[@]}"; do
    tap_name="$(printf '%s' "${tap_csv_item}" | xargs)"
    [ -n "${tap_name}" ] || continue
    if [ -z "${tap_seen[${tap_name}]+x}" ]; then
      taps+=("${tap_name}")
      tap_seen["${tap_name}"]=1
    fi
  done

  if [ -f "${BROKER_INVENTORY_SCRIPT}" ]; then
    # shellcheck disable=SC1090
    . "${BROKER_INVENTORY_SCRIPT}"
    if declare -F inventory_taps >/dev/null 2>&1; then
      while IFS= read -r tap_name; do
        [ -n "${tap_name}" ] || continue
        if [ -z "${tap_seen[${tap_name}]+x}" ]; then
          taps+=("${tap_name}")
          tap_seen["${tap_name}"]=1
        fi
      done < <(inventory_taps)
    fi
  fi

  printf '%s\n' "${taps[@]}"
}

remove_nat_rule() {
  if [ -z "${UPLINK_IFACE}" ]; then
    return
  fi

  iptables -t nat -C POSTROUTING -s "${GUEST_CIDR}" -o "${UPLINK_IFACE}" -j MASQUERADE 2>/dev/null && \
    iptables -t nat -D POSTROUTING -s "${GUEST_CIDR}" -o "${UPLINK_IFACE}" -j MASQUERADE || true
}

main() {
  require_root
  auto_detect_uplink
  remove_nat_rule

  mapfile -t taps < <(collect_taps)
  for tap_name in "${taps[@]}"; do
    ip link del "${tap_name}" 2>/dev/null || true
  done

  ip link del "${BRIDGE_NAME}" 2>/dev/null || true
  log "firecracker bridge network removed"
}

main "$@"
