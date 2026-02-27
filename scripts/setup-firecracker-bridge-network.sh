#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly DEFAULT_BRIDGE_NAME="fcbr0"
readonly DEFAULT_BRIDGE_CIDR="172.30.0.1/24"
readonly DEFAULT_GUEST_CIDR="172.30.0.0/24"
readonly DEFAULT_TAP_LIST="tap-relay,tap-kafka"
readonly DEFAULT_TAP_OWNER="root"
readonly DEFAULT_BROKER_INVENTORY_SCRIPT="${REPO_ROOT}/firecracker/runtime/broker_inventory.sh"

BRIDGE_NAME="${FIRECRACKER_BRIDGE_NAME:-${DEFAULT_BRIDGE_NAME}}"
BRIDGE_CIDR="${FIRECRACKER_BRIDGE_CIDR:-${DEFAULT_BRIDGE_CIDR}}"
GUEST_CIDR="${FIRECRACKER_GUEST_CIDR:-${DEFAULT_GUEST_CIDR}}"
TAP_LIST="${FIRECRACKER_TAP_LIST:-${DEFAULT_TAP_LIST}}"
TAP_OWNER="${FIRECRACKER_TAP_OWNER:-${DEFAULT_TAP_OWNER}}"
UPLINK_IFACE="${FIRECRACKER_UPLINK_IFACE:-}"
BROKER_INVENTORY_SCRIPT="${FIRECRACKER_BROKER_INVENTORY_SCRIPT:-${DEFAULT_BROKER_INVENTORY_SCRIPT}}"

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    die "run as root"
  fi
}

require_cmd() {
  local command_name="$1"
  command -v "${command_name}" >/dev/null 2>&1 || die "missing command: ${command_name}"
}

auto_detect_uplink() {
  if [ -n "${UPLINK_IFACE}" ]; then
    return
  fi

  UPLINK_IFACE="$(ip route show default 2>/dev/null | awk '/default/ {print $5; exit}')"
  [ -n "${UPLINK_IFACE}" ] || die "could not auto-detect FIRECRACKER_UPLINK_IFACE"
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

ensure_bridge() {
  local bridge_ip="${BRIDGE_CIDR%%/*}"

  if ! ip link show "${BRIDGE_NAME}" >/dev/null 2>&1; then
    ip link add "${BRIDGE_NAME}" type bridge
  fi

  if ! ip addr show dev "${BRIDGE_NAME}" | grep -q "${bridge_ip}"; then
    ip addr add "${BRIDGE_CIDR}" dev "${BRIDGE_NAME}"
  fi

  ip link set "${BRIDGE_NAME}" up
}

ensure_tap() {
  local tap_name="$1"

  if ! ip link show "${tap_name}" >/dev/null 2>&1; then
    ip tuntap add dev "${tap_name}" mode tap user "${TAP_OWNER}"
  fi

  ip link set "${tap_name}" master "${BRIDGE_NAME}"
  ip link set "${tap_name}" up
}

ensure_nat() {
  sysctl -w net.ipv4.ip_forward=1 >/dev/null

  iptables -C FORWARD -i "${BRIDGE_NAME}" -o "${UPLINK_IFACE}" -j ACCEPT 2>/dev/null || \
    iptables -A FORWARD -i "${BRIDGE_NAME}" -o "${UPLINK_IFACE}" -j ACCEPT

  iptables -C FORWARD -i "${UPLINK_IFACE}" -o "${BRIDGE_NAME}" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || \
    iptables -A FORWARD -i "${UPLINK_IFACE}" -o "${BRIDGE_NAME}" -m state --state RELATED,ESTABLISHED -j ACCEPT

  iptables -t nat -C POSTROUTING -s "${GUEST_CIDR}" -o "${UPLINK_IFACE}" -j MASQUERADE 2>/dev/null || \
    iptables -t nat -A POSTROUTING -s "${GUEST_CIDR}" -o "${UPLINK_IFACE}" -j MASQUERADE
}

main() {
  require_root
  require_cmd ip
  require_cmd iptables
  require_cmd sysctl

  auto_detect_uplink
  ensure_bridge

  mapfile -t taps < <(collect_taps)
  for tap_name in "${taps[@]}"; do
    ensure_tap "${tap_name}"
  done

  ensure_nat

  log "firecracker bridge network ready"
  log "bridge=${BRIDGE_NAME} bridge_cidr=${BRIDGE_CIDR} guest_cidr=${GUEST_CIDR} uplink=${UPLINK_IFACE}"
  log "taps=${taps[*]}"
}

main "$@"
