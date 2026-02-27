#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_CHROOT_BASE="/srv/jailer"
readonly DEFAULT_FIRECRACKER_EXEC="/usr/local/bin/firecracker"
readonly DEFAULT_VM_SOCKET_DIR="/tmp"
readonly DEFAULT_PARENT_CGROUP="firecracker"

VM_ID="${1:-}"
SOCKET_PATH_INPUT="${2:-}"

CHROOT_BASE="${FIRECRACKER_CHROOT_BASE:-${DEFAULT_CHROOT_BASE}}"
FIRECRACKER_EXEC="${FIRECRACKER_EXEC:-${DEFAULT_FIRECRACKER_EXEC}}"
PARENT_CGROUP="${FIRECRACKER_JAILER_PARENT_CGROUP:-${DEFAULT_PARENT_CGROUP}}"

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

resolve_socket_path() {
  local vm_id="$1"
  local provided_path="$2"
  local default_socket_path="${DEFAULT_VM_SOCKET_DIR}/${vm_id}-fc.sock"

  if [ -n "${provided_path}" ]; then
    printf '%s' "${provided_path}"
    return
  fi

  case "${vm_id}" in
    kafka)
      printf '%s' "${DEFAULT_VM_SOCKET_DIR}/kafka-fc.sock"
      ;;
    relay|webhook-relay)
      printf '%s' "${DEFAULT_VM_SOCKET_DIR}/relay-fc.sock"
      ;;
    *)
      printf '%s' "${default_socket_path}"
      ;;
  esac
}

main() {
  [ -n "${VM_ID}" ] || die "usage: cleanup.sh <vm_id> [socket_path]"

  local socket_path=""
  local jail_exec_basename=""
  local jail_root=""
  local mount_point=""
  local cgroup_path=""

  socket_path="$(resolve_socket_path "${VM_ID}" "${SOCKET_PATH_INPUT}")"
  jail_exec_basename="$(basename "${FIRECRACKER_EXEC}")"
  jail_root="${CHROOT_BASE}/${jail_exec_basename}/${VM_ID}/root"
  cgroup_path="/sys/fs/cgroup/${PARENT_CGROUP}/${VM_ID}/cgroup.kill"

  if [ -L "${socket_path}" ] || [ -S "${socket_path}" ]; then
    rm -f "${socket_path}"
  fi

  if [ -d "${jail_root}" ]; then
    mapfile -t mounts < <(findmnt -R -n -o TARGET "${jail_root}" 2>/dev/null | sort -r)
    for mount_point in "${mounts[@]}"; do
      umount -l "${mount_point}" || true
    done
    rm -rf "${jail_root}"
  fi

  if [ -e "${cgroup_path}" ]; then
    printf '1' > "${cgroup_path}" || true
  fi
}

main "$@"
