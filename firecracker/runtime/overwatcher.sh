#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_SOCKET_LIST="/tmp/kafka-fc.sock,/tmp/relay-fc.sock"
readonly DEFAULT_LOG_DIR="/var/log/firecracker"
readonly DEFAULT_FALLBACK_LOG_DIR="/tmp/firecracker"
readonly DEFAULT_LOG_FILE_NAME="overwatcher.log"
readonly DEFAULT_POLL_SECONDS=10
readonly DEFAULT_HEALTH_TIMEOUT_SECONDS=2

SOCKET_LIST="${FIRECRACKER_WATCHDOG_SOCKETS:-${DEFAULT_SOCKET_LIST}}"
LOG_DIR="${FIRECRACKER_WATCHDOG_LOG_DIR:-${DEFAULT_LOG_DIR}}"
FALLBACK_LOG_DIR="${FIRECRACKER_WATCHDOG_FALLBACK_LOG_DIR:-${DEFAULT_FALLBACK_LOG_DIR}}"
LOG_FILE_NAME="${FIRECRACKER_WATCHDOG_LOG_FILE:-${DEFAULT_LOG_FILE_NAME}}"
POLL_SECONDS="${FIRECRACKER_WATCHDOG_INTERVAL_SECONDS:-${DEFAULT_POLL_SECONDS}}"
HEALTH_TIMEOUT_SECONDS="${FIRECRACKER_WATCHDOG_TIMEOUT_SECONDS:-${DEFAULT_HEALTH_TIMEOUT_SECONDS}}"

LOG_FILE_PATH=""

ensure_writable_dir() {
  local directory_path="$1"

  mkdir -p "${directory_path}" >/dev/null 2>&1 || return 1
  touch "${directory_path}/.write-test" >/dev/null 2>&1 || return 1
  rm -f "${directory_path}/.write-test" >/dev/null 2>&1 || true
  return 0
}

resolve_log_dir() {
  if ensure_writable_dir "${LOG_DIR}"; then
    printf '%s\n' "${LOG_DIR}"
    return
  fi

  ensure_writable_dir "${FALLBACK_LOG_DIR}" || {
    printf 'error: unable to write logs in %s or %s\n' "${LOG_DIR}" "${FALLBACK_LOG_DIR}" >&2
    exit 1
  }
  printf '%s\n' "${FALLBACK_LOG_DIR}"
}

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "${LOG_FILE_PATH}" >&2
}

check_socket_health() {
  local socket_path="$1"
  local process_id=""

  if [ ! -S "${socket_path}" ]; then
    return 0
  fi

  process_id="$(lsof -t "${socket_path}" 2>/dev/null | head -n 1 || true)"
  if [ -z "${process_id}" ]; then
    return 0
  fi

  if timeout "${HEALTH_TIMEOUT_SECONDS}" curl -s --unix-socket "${socket_path}" http://localhost/ >/dev/null 2>&1; then
    return 0
  fi

  log "ALERT: Firecracker API unresponsive for socket=${socket_path} pid=${process_id}; killing process"
  kill -9 "${process_id}" || true
}

main() {
  local resolved_log_dir=""

  resolved_log_dir="$(resolve_log_dir)"
  LOG_FILE_PATH="${resolved_log_dir}/${LOG_FILE_NAME}"

  IFS=',' read -r -a sockets <<< "${SOCKET_LIST}"
  while true; do
    for socket_path in "${sockets[@]}"; do
      check_socket_health "${socket_path}"
    done
    sleep "${POLL_SECONDS}"
  done
}

main "$@"
