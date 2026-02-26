#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly DEFAULT_CONFIG_PATH="firecracker/firecracker-config.template.json"
readonly DEFAULT_LAUNCHER_PATH="${REPO_ROOT}/firecracker/runtime/launch.sh"
readonly DEFAULT_LAUNCHER_PROFILE="relay"
readonly DEFAULT_FALLBACK_LOG_DIR="/tmp/firecracker"
readonly FLAG_FALSE=0
readonly FLAG_TRUE=1

CONFIG_PATH="${DEFAULT_CONFIG_PATH}"
SOCKET_PATH=""
LAUNCHER_PATH="${FIRECRACKER_LAUNCHER_PATH:-${DEFAULT_LAUNCHER_PATH}}"
LAUNCHER_PROFILE="${FIRECRACKER_LAUNCHER_PROFILE:-${DEFAULT_LAUNCHER_PROFILE}}"
FALLBACK_LOG_DIR="${FIRECRACKER_FALLBACK_LOG_DIR:-${DEFAULT_FALLBACK_LOG_DIR}}"
USE_LAUNCHER_IF_AVAILABLE=${FLAG_TRUE}
LAUNCHER_PATH_FROM_USER=${FLAG_FALSE}
RUNTIME_CONFIG_PATH=""
RUNTIME_CONFIG_TEMP_PATH=""

if [ -n "${FIRECRACKER_LAUNCHER_PATH:-}" ]; then
  LAUNCHER_PATH_FROM_USER=${FLAG_TRUE}
fi

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
  --config <path>            Firecracker config JSON path (default: firecracker/firecracker-config.template.json)
  --socket <path>            API socket path (optional)
  --launcher <path>          Launcher script path (optional, defaults to repo runtime path)
  --launcher-profile <name>  Launcher profile/name argument (default: relay)
  --fallback-log-dir <path>  Fallback log directory (default: /tmp/firecracker)
  --no-launcher              Force direct Firecracker execution
EOF_USAGE
}

require_option_value() {
  local option_name="$1"

  [ "$#" -ge 2 ] || die "missing value for ${option_name}"
}

cleanup_runtime_config() {
  if [ -n "${RUNTIME_CONFIG_TEMP_PATH}" ] && [ -f "${RUNTIME_CONFIG_TEMP_PATH}" ]; then
    rm -f "${RUNTIME_CONFIG_TEMP_PATH}"
  fi
}

extract_config_log_path() {
  local source_config_path="$1"

  sed -nE 's/^[[:space:]]*"log_path"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/p' "${source_config_path}" | head -n 1
}

ensure_directory_writable() {
  local target_directory="$1"

  mkdir -p "${target_directory}" >/dev/null 2>&1 || return 1
  [ -w "${target_directory}" ]
}

resolve_log_directory() {
  local requested_log_directory="$1"

  if ensure_directory_writable "${requested_log_directory}"; then
    printf '%s' "${requested_log_directory}"
    return 0
  fi

  log "warning: log directory ${requested_log_directory} is not writable; falling back to ${FALLBACK_LOG_DIR}"

  ensure_directory_writable "${FALLBACK_LOG_DIR}" || \
    die "failed to create writable fallback log directory: ${FALLBACK_LOG_DIR}"

  printf '%s' "${FALLBACK_LOG_DIR}"
}

rewrite_config_log_path() {
  local source_config_path="$1"
  local target_log_path="$2"
  local output_config_path="$3"

  awk -v runtime_log_path="${target_log_path}" '
    BEGIN {
      log_path_rewritten = 0
    }
    /"log_path"[[:space:]]*:/ && log_path_rewritten == 0 {
      sub(/"log_path"[[:space:]]*:[[:space:]]*"[^"]*"/, "\"log_path\": \"" runtime_log_path "\"")
      log_path_rewritten = 1
    }
    {
      print
    }
    END {
      if (log_path_rewritten == 0) {
        exit 1
      }
    }
  ' "${source_config_path}" >"${output_config_path}" || \
    die "failed to rewrite log_path in runtime config: ${source_config_path}"
}

prepare_runtime_config() {
  local configured_log_path=""
  local configured_log_directory=""
  local resolved_log_directory=""
  local runtime_log_path=""

  RUNTIME_CONFIG_PATH="${CONFIG_PATH}"
  configured_log_path="$(extract_config_log_path "${CONFIG_PATH}")"

  if [ -z "${configured_log_path}" ]; then
    return
  fi

  configured_log_directory="$(dirname "${configured_log_path}")"
  resolved_log_directory="$(resolve_log_directory "${configured_log_directory}")"

  if [ "${configured_log_directory}" = "${resolved_log_directory}" ]; then
    return
  fi

  runtime_log_path="${resolved_log_directory}/$(basename "${configured_log_path}")"
  RUNTIME_CONFIG_TEMP_PATH="$(mktemp "${TMPDIR:-/tmp}/firecracker-config.XXXXXX.json")"
  rewrite_config_log_path "${CONFIG_PATH}" "${runtime_log_path}" "${RUNTIME_CONFIG_TEMP_PATH}"
  RUNTIME_CONFIG_PATH="${RUNTIME_CONFIG_TEMP_PATH}"
  log "using runtime config ${RUNTIME_CONFIG_PATH} with fallback log path ${runtime_log_path}"
}

should_use_launcher() {
  if [ "${USE_LAUNCHER_IF_AVAILABLE}" -ne "${FLAG_TRUE}" ]; then
    return 1
  fi

  if [ -x "${LAUNCHER_PATH}" ]; then
    return 0
  fi

  if [ "${LAUNCHER_PATH_FROM_USER}" -eq "${FLAG_TRUE}" ]; then
    die "configured launcher is not executable: ${LAUNCHER_PATH}"
  fi

  log "launcher not found at ${LAUNCHER_PATH}; falling back to firecracker binary in PATH"
  return 1
}

start_with_launcher() {
  if [ -n "${SOCKET_PATH}" ]; then
    log "starting firecracker via launcher ${LAUNCHER_PATH} with config ${RUNTIME_CONFIG_PATH} and socket ${SOCKET_PATH}"
    exec "${LAUNCHER_PATH}" "${LAUNCHER_PROFILE}" "${RUNTIME_CONFIG_PATH}" "${SOCKET_PATH}"
  fi

  log "starting firecracker via launcher ${LAUNCHER_PATH} with config ${RUNTIME_CONFIG_PATH}"
  exec "${LAUNCHER_PATH}" "${LAUNCHER_PROFILE}" "${RUNTIME_CONFIG_PATH}"
}

start_with_firecracker() {
  command -v firecracker >/dev/null 2>&1 || die "firecracker binary not found"

  if [ -n "${SOCKET_PATH}" ]; then
    log "starting firecracker with config ${RUNTIME_CONFIG_PATH} and socket ${SOCKET_PATH}"
    exec firecracker --config-file "${RUNTIME_CONFIG_PATH}" --api-sock "${SOCKET_PATH}"
  fi

  log "starting firecracker with config ${RUNTIME_CONFIG_PATH}"
  exec firecracker --config-file "${RUNTIME_CONFIG_PATH}"
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --config)
        require_option_value "$@"
        CONFIG_PATH="$2"
        shift 2
        ;;
      --socket)
        require_option_value "$@"
        SOCKET_PATH="$2"
        shift 2
        ;;
      --launcher)
        require_option_value "$@"
        LAUNCHER_PATH="$2"
        LAUNCHER_PATH_FROM_USER=${FLAG_TRUE}
        shift 2
        ;;
      --launcher-profile)
        require_option_value "$@"
        LAUNCHER_PROFILE="$2"
        shift 2
        ;;
      --fallback-log-dir)
        require_option_value "$@"
        FALLBACK_LOG_DIR="$2"
        shift 2
        ;;
      --no-launcher)
        USE_LAUNCHER_IF_AVAILABLE=${FLAG_FALSE}
        shift 1
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
  trap cleanup_runtime_config EXIT
  parse_args "$@"

  [ -f "${CONFIG_PATH}" ] || die "config file not found: ${CONFIG_PATH}"

  if [ ! -e /dev/kvm ]; then
    die "/dev/kvm not found; host does not support KVM for Firecracker"
  fi

  prepare_runtime_config

  if should_use_launcher; then
    start_with_launcher
  fi

  start_with_firecracker
}

main "$@"
