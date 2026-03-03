#!/usr/bin/env bash
# Collect persistent kernel panic logs from /sys/fs/pstore at boot.
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_ENV_FILE="/etc/firecracker/pstore-collect.env"
readonly DEFAULT_PSTORE_DIR="/sys/fs/pstore"
readonly DEFAULT_OUT_BASE="/var/log/firecracker/pstore"
readonly DEFAULT_OUT_LOG="/var/log/firecracker/kernel-pstore.log"
readonly DEFAULT_FALLBACK_OUT_BASE="/tmp/firecracker-watchdog/pstore"
readonly DEFAULT_FALLBACK_OUT_LOG="/tmp/firecracker-watchdog/kernel-pstore.log"
readonly DEFAULT_CLEAR_AFTER_COPY="true"

PSTORE_ENV_FILE="${PSTORE_ENV_FILE:-${DEFAULT_ENV_FILE}}"
if [ -f "${PSTORE_ENV_FILE}" ]; then
  # shellcheck disable=SC1090
  . "${PSTORE_ENV_FILE}"
fi

PSTORE_DIR="${PSTORE_DIR:-${DEFAULT_PSTORE_DIR}}"
PSTORE_OUT_BASE="${PSTORE_OUT_BASE:-${DEFAULT_OUT_BASE}}"
PSTORE_OUT_LOG="${PSTORE_OUT_LOG:-${DEFAULT_OUT_LOG}}"
PSTORE_CLEAR_AFTER_COPY="${PSTORE_CLEAR_AFTER_COPY:-${DEFAULT_CLEAR_AFTER_COPY}}"

as_bool() {
  local normalized_value=""

  normalized_value="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  case "${normalized_value}" in
    1|true|yes|on) return 0 ;;
    *) return 1 ;;
  esac
}

ensure_writable_dir() {
  local directory_path="$1"

  mkdir -p "${directory_path}" >/dev/null 2>&1 || return 1
  touch "${directory_path}/.write-test" >/dev/null 2>&1 || return 1
  rm -f "${directory_path}/.write-test" >/dev/null 2>&1 || true
  return 0
}

ensure_writable_file() {
  local file_path="$1"
  local parent_dir=""

  parent_dir="$(dirname "${file_path}")"
  mkdir -p "${parent_dir}" >/dev/null 2>&1 || return 1
  touch "${file_path}" >/dev/null 2>&1 || return 1
  return 0
}

resolve_output_paths() {
  if ! ensure_writable_dir "${PSTORE_OUT_BASE}"; then
    PSTORE_OUT_BASE="${DEFAULT_FALLBACK_OUT_BASE}"
    ensure_writable_dir "${PSTORE_OUT_BASE}" || {
      printf 'error: unable to write pstore output base\n' >&2
      exit 1
    }
  fi

  if ! ensure_writable_file "${PSTORE_OUT_LOG}"; then
    PSTORE_OUT_LOG="${DEFAULT_FALLBACK_OUT_LOG}"
    ensure_writable_file "${PSTORE_OUT_LOG}" || {
      printf 'error: unable to write pstore log file\n' >&2
      exit 1
    }
  fi
}

main() {
  local stamp=""
  local run_dir=""
  local source_file=""
  local file_name=""
  local target_file=""

  [ -d "${PSTORE_DIR}" ] || exit 0

  mapfile -t pstore_files < <(find "${PSTORE_DIR}" -maxdepth 1 -type f 2>/dev/null | sort)
  if [ "${#pstore_files[@]}" -eq 0 ]; then
    exit 0
  fi

  resolve_output_paths

  stamp="$(date -u '+%Y%m%dT%H%M%SZ')"
  run_dir="${PSTORE_OUT_BASE}/${stamp}"
  mkdir -p "${run_dir}"

  {
    printf '[%s] pstore-collect start files=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "${#pstore_files[@]}"

    for source_file in "${pstore_files[@]}"; do
      file_name="$(basename "${source_file}")"
      target_file="${run_dir}/${file_name}"

      cp -a "${source_file}" "${target_file}" 2>/dev/null || cp "${source_file}" "${target_file}" 2>/dev/null || true
      printf -- '--- begin %s ---\n' "${file_name}"
      cat "${source_file}" || true
      printf -- '--- end %s ---\n' "${file_name}"

      if as_bool "${PSTORE_CLEAR_AFTER_COPY}"; then
        rm -f "${source_file}" || true
      fi
    done

    printf '[%s] pstore-collect complete output=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "${run_dir}"
  } >> "${PSTORE_OUT_LOG}"
}

main "$@"
