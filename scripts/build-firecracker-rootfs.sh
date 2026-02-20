#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_ROOTFS_SIZE_MB=256
readonly DEFAULT_DATA_SIZE_MB=1024
readonly DEFAULT_ROOTFS_PATH="out/firecracker/rootfs.ext4"
readonly DEFAULT_DATA_PATH="out/firecracker/data.ext4"
readonly DEFAULT_BINARY_PATH="target/release/webhook-relay"

BINARY_PATH="${DEFAULT_BINARY_PATH}"
ROOTFS_PATH="${DEFAULT_ROOTFS_PATH}"
DATA_PATH="${DEFAULT_DATA_PATH}"
ROOTFS_SIZE_MB="${DEFAULT_ROOTFS_SIZE_MB}"
DATA_SIZE_MB="${DEFAULT_DATA_SIZE_MB}"

MOUNT_DIR=""

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

usage() {
  cat <<'EOF_USAGE' >&2
Usage: scripts/build-firecracker-rootfs.sh [options]

Options:
  --binary <path>      Relay binary path (default: target/release/webhook-relay)
  --rootfs <path>      Rootfs image output path (default: out/firecracker/rootfs.ext4)
  --data <path>        Data image output path (default: out/firecracker/data.ext4)
  --rootfs-mb <size>   Rootfs size in MiB (default: 256)
  --data-mb <size>     Data image size in MiB (default: 1024)
EOF_USAGE
}

cleanup() {
  if [ -n "${MOUNT_DIR}" ] && mount | grep -q "on ${MOUNT_DIR} "; then
    sudo umount "${MOUNT_DIR}" || true
  fi
  if [ -n "${MOUNT_DIR}" ] && [ -d "${MOUNT_DIR}" ]; then
    rmdir "${MOUNT_DIR}" || true
  fi
}

trap cleanup EXIT INT TERM

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --binary)
        BINARY_PATH="$2"
        shift 2
        ;;
      --rootfs)
        ROOTFS_PATH="$2"
        shift 2
        ;;
      --data)
        DATA_PATH="$2"
        shift 2
        ;;
      --rootfs-mb)
        ROOTFS_SIZE_MB="$2"
        shift 2
        ;;
      --data-mb)
        DATA_SIZE_MB="$2"
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

require_cmd() {
  local cmd="$1"
  command -v "${cmd}" >/dev/null 2>&1 || die "missing required command: ${cmd}"
}

main() {
  parse_args "$@"

  require_cmd "truncate"
  require_cmd "mkfs.ext4"
  require_cmd "sudo"

  [ -f "${BINARY_PATH}" ] || die "binary not found: ${BINARY_PATH}"

  mkdir -p "$(dirname "${ROOTFS_PATH}")"
  mkdir -p "$(dirname "${DATA_PATH}")"

  log "creating rootfs image: ${ROOTFS_PATH} (${ROOTFS_SIZE_MB} MiB)"
  truncate -s "${ROOTFS_SIZE_MB}M" "${ROOTFS_PATH}"
  mkfs.ext4 -F "${ROOTFS_PATH}" >/dev/null

  MOUNT_DIR="$(mktemp -d)"
  sudo mount -o loop "${ROOTFS_PATH}" "${MOUNT_DIR}"

  sudo install -m 0755 "${BINARY_PATH}" "${MOUNT_DIR}/init"
  sudo mkdir -p "${MOUNT_DIR}/etc"
  sudo tee "${MOUNT_DIR}/etc/hostname" >/dev/null <<'EOF_HOSTNAME'
webhook-relay
EOF_HOSTNAME

  sudo umount "${MOUNT_DIR}"
  rmdir "${MOUNT_DIR}"
  MOUNT_DIR=""

  log "creating data image: ${DATA_PATH} (${DATA_SIZE_MB} MiB)"
  truncate -s "${DATA_SIZE_MB}M" "${DATA_PATH}"
  mkfs.ext4 -F "${DATA_PATH}" >/dev/null

  log "firecracker images created"
}

main "$@"
