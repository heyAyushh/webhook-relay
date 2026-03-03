#!/usr/bin/env bash
# Run from a separate host to perform tunnel checks via chisel.
set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

readonly DEFAULT_ENV_FILE="/etc/firecracker/chisel-check.env"
readonly DEFAULT_ALERT_HELPER_SCRIPT="${SCRIPT_DIR}/alert.sh"
readonly DEFAULT_LOG_FILE="/var/log/firecracker/external-chisel-check.log"
readonly DEFAULT_FALLBACK_LOG_FILE="/tmp/firecracker-watchdog/external-chisel-check.log"
readonly DEFAULT_KAFKA_REMOTE="172.30.0.10:9092"
readonly DEFAULT_RELAY_REMOTE="172.30.0.20:8080"
readonly DEFAULT_LOCAL_KAFKA_PORT="19092"
readonly DEFAULT_LOCAL_RELAY_PORT="18080"
readonly DEFAULT_ENABLE_KAFKA="true"
readonly DEFAULT_ENABLE_RELAY="true"
readonly DEFAULT_RELAY_HEALTH_PATH="/health"
readonly DEFAULT_EXPECT_RELAY_CODE="200"
readonly DEFAULT_CONNECT_WAIT_SECONDS="8"
readonly DEFAULT_TIMEOUT_SECONDS="10"

CHISEL_CHECK_ENV_FILE="${CHISEL_CHECK_ENV_FILE:-${DEFAULT_ENV_FILE}}"
if [ -f "${CHISEL_CHECK_ENV_FILE}" ]; then
  # shellcheck disable=SC1090
  . "${CHISEL_CHECK_ENV_FILE}"
fi

ALERT_HELPER_SCRIPT="${FIRECRACKER_WATCHDOG_ALERT_HELPER:-${DEFAULT_ALERT_HELPER_SCRIPT}}"
if [ -f "${ALERT_HELPER_SCRIPT}" ]; then
  # shellcheck disable=SC1090
  . "${ALERT_HELPER_SCRIPT}"
fi

if ! declare -F alert_emit >/dev/null 2>&1; then
  alert_emit() {
    :
  }
fi

CHISEL_CHECK_URL="${CHISEL_CHECK_URL:-}"
CHISEL_CHECK_AUTH="${CHISEL_CHECK_AUTH:-}"
CHISEL_CHECK_KAFKA_REMOTE="${CHISEL_CHECK_KAFKA_REMOTE:-${DEFAULT_KAFKA_REMOTE}}"
CHISEL_CHECK_RELAY_REMOTE="${CHISEL_CHECK_RELAY_REMOTE:-${DEFAULT_RELAY_REMOTE}}"
CHISEL_CHECK_LOCAL_KAFKA_PORT="${CHISEL_CHECK_LOCAL_KAFKA_PORT:-${DEFAULT_LOCAL_KAFKA_PORT}}"
CHISEL_CHECK_LOCAL_RELAY_PORT="${CHISEL_CHECK_LOCAL_RELAY_PORT:-${DEFAULT_LOCAL_RELAY_PORT}}"
CHISEL_CHECK_ENABLE_KAFKA="${CHISEL_CHECK_ENABLE_KAFKA:-${DEFAULT_ENABLE_KAFKA}}"
CHISEL_CHECK_ENABLE_RELAY="${CHISEL_CHECK_ENABLE_RELAY:-${DEFAULT_ENABLE_RELAY}}"
CHISEL_CHECK_RELAY_HEALTH_PATH="${CHISEL_CHECK_RELAY_HEALTH_PATH:-${DEFAULT_RELAY_HEALTH_PATH}}"
CHISEL_CHECK_EXPECT_RELAY_CODE="${CHISEL_CHECK_EXPECT_RELAY_CODE:-${DEFAULT_EXPECT_RELAY_CODE}}"
CHISEL_CHECK_CONNECT_WAIT_SECONDS="${CHISEL_CHECK_CONNECT_WAIT_SECONDS:-${DEFAULT_CONNECT_WAIT_SECONDS}}"
CHISEL_CHECK_TIMEOUT_SECONDS="${CHISEL_CHECK_TIMEOUT_SECONDS:-${DEFAULT_TIMEOUT_SECONDS}}"
CHISEL_CHECK_LOG_FILE="${CHISEL_CHECK_LOG_FILE:-${DEFAULT_LOG_FILE}}"

resolve_log_file() {
  local candidate_file="$1"
  local fallback_file="$2"

  mkdir -p "$(dirname "${candidate_file}")" >/dev/null 2>&1 && touch "${candidate_file}" >/dev/null 2>&1 && {
    printf '%s\n' "${candidate_file}"
    return
  }

  mkdir -p "$(dirname "${fallback_file}")" >/dev/null 2>&1
  touch "${fallback_file}" >/dev/null 2>&1 || true
  printf '%s\n' "${fallback_file}"
}

CHISEL_CHECK_LOG_FILE="$(resolve_log_file "${CHISEL_CHECK_LOG_FILE}" "${DEFAULT_FALLBACK_LOG_FILE}")"

log() {
  local now_text=""

  now_text="$(date '+%Y-%m-%d %H:%M:%S')"
  printf '[%s] %s\n' "${now_text}" "$1" | tee -a "${CHISEL_CHECK_LOG_FILE}" >&2
}

as_bool() {
  local normalized_value=""

  normalized_value="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  case "${normalized_value}" in
    1|true|yes|on) return 0 ;;
    *) return 1 ;;
  esac
}

wait_for_port() {
  local host_name="$1"
  local port_number="$2"
  local wait_seconds="$3"
  local i=""

  for i in $(seq 1 "${wait_seconds}"); do
    if nc -z -w 1 "${host_name}" "${port_number}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

http_code() {
  local target_url="$1"
  local code=""

  code="$(curl -sS -m "${CHISEL_CHECK_TIMEOUT_SECONDS}" -o /dev/null -w '%{http_code}' "${target_url}" 2>/dev/null || true)"
  if [ -z "${code}" ]; then
    code="000"
  fi
  printf '%s\n' "${code}"
}

run_kafka_check() {
  local endpoint="$1"

  if command -v timeout >/dev/null 2>&1; then
    timeout "${CHISEL_CHECK_TIMEOUT_SECONDS}" kcat -b "${endpoint}" -L >/dev/null 2>&1
    return $?
  fi

  kcat -b "${endpoint}" -L >/dev/null 2>&1
}

main() {
  local check_kafka=0
  local check_relay=0
  local failed=0
  local relay_code="skip"
  local kafka_result="skip"
  local chisel_stderr=""
  local chisel_pid=""
  local tail_reason=""

  if ! command -v chisel >/dev/null 2>&1; then
    log "FAIL: chisel binary not found on checker host"
    alert_emit critical "external_chisel_binary_missing" "chisel binary not found on checker host"
    exit 1
  fi

  if ! command -v nc >/dev/null 2>&1; then
    log "FAIL: nc binary not found on checker host"
    alert_emit critical "external_chisel_nc_missing" "nc binary not found on checker host"
    exit 1
  fi

  if [ -z "${CHISEL_CHECK_URL}" ] || [ -z "${CHISEL_CHECK_AUTH}" ]; then
    log "FAIL: CHISEL_CHECK_URL and CHISEL_CHECK_AUTH are required"
    exit 2
  fi

  if as_bool "${CHISEL_CHECK_ENABLE_KAFKA}"; then
    check_kafka=1
  fi
  if as_bool "${CHISEL_CHECK_ENABLE_RELAY}"; then
    check_relay=1
  fi

  if [ "${check_kafka}" -eq 0 ] && [ "${check_relay}" -eq 0 ]; then
    log "FAIL: both CHISEL_CHECK_ENABLE_KAFKA and CHISEL_CHECK_ENABLE_RELAY are disabled"
    alert_emit critical "external_chisel_no_checks" "chisel checker has no enabled checks"
    exit 1
  fi

  if [ "${check_kafka}" -eq 1 ] && ! command -v kcat >/dev/null 2>&1; then
    log "FAIL: kcat binary not found; Kafka tunnel check requires kcat"
    alert_emit critical "external_chisel_kcat_missing" "kcat binary not found on checker host"
    exit 1
  fi

  forwards=()
  if [ "${check_kafka}" -eq 1 ]; then
    forwards+=("${CHISEL_CHECK_LOCAL_KAFKA_PORT}:${CHISEL_CHECK_KAFKA_REMOTE}")
  fi
  if [ "${check_relay}" -eq 1 ]; then
    forwards+=("${CHISEL_CHECK_LOCAL_RELAY_PORT}:${CHISEL_CHECK_RELAY_REMOTE}")
  fi

  chisel_stderr="$(mktemp)"
  cleanup() {
    if [ -n "${chisel_pid:-}" ]; then
      kill "${chisel_pid}" >/dev/null 2>&1 || true
      wait "${chisel_pid}" >/dev/null 2>&1 || true
    fi
    rm -f "${chisel_stderr}"
  }
  trap cleanup EXIT INT TERM

  chisel client --auth "${CHISEL_CHECK_AUTH}" "${CHISEL_CHECK_URL}" "${forwards[@]}" >"${chisel_stderr}" 2>&1 &
  chisel_pid=$!
  sleep 1

  if ! kill -0 "${chisel_pid}" >/dev/null 2>&1; then
    tail_reason="$(tail -n 5 "${chisel_stderr}" | tr '\n' ';')"
    log "FAIL: chisel tunnel process exited immediately: ${tail_reason}"
    alert_emit critical "external_chisel_tunnel_start_failed" "chisel tunnel failed to start (${CHISEL_CHECK_URL})"
    exit 1
  fi

  if [ "${check_relay}" -eq 1 ]; then
    if wait_for_port 127.0.0.1 "${CHISEL_CHECK_LOCAL_RELAY_PORT}" "${CHISEL_CHECK_CONNECT_WAIT_SECONDS}"; then
      relay_code="$(http_code "http://127.0.0.1:${CHISEL_CHECK_LOCAL_RELAY_PORT}${CHISEL_CHECK_RELAY_HEALTH_PATH}")"
      if [ "${relay_code}" != "${CHISEL_CHECK_EXPECT_RELAY_CODE}" ]; then
        failed=1
        log "FAIL: relay via chisel returned ${relay_code} (expected ${CHISEL_CHECK_EXPECT_RELAY_CODE})"
      fi
    else
      failed=1
      relay_code="timeout"
      log "FAIL: relay chisel local port ${CHISEL_CHECK_LOCAL_RELAY_PORT} did not open"
    fi
  fi

  if [ "${check_kafka}" -eq 1 ]; then
    if wait_for_port 127.0.0.1 "${CHISEL_CHECK_LOCAL_KAFKA_PORT}" "${CHISEL_CHECK_CONNECT_WAIT_SECONDS}"; then
      if run_kafka_check "127.0.0.1:${CHISEL_CHECK_LOCAL_KAFKA_PORT}"; then
        kafka_result="ok"
      else
        failed=1
        kafka_result="metadata_failed"
        log "FAIL: kafka metadata query failed via chisel tunnel"
      fi
    else
      failed=1
      kafka_result="port_timeout"
      log "FAIL: kafka chisel local port ${CHISEL_CHECK_LOCAL_KAFKA_PORT} did not open"
    fi
  fi

  if [ "${failed}" -eq 0 ]; then
    log "OK: external chisel checks passed (relay=${relay_code} kafka=${kafka_result} url=${CHISEL_CHECK_URL})"
    exit 0
  fi

  tail_reason="$(tail -n 8 "${chisel_stderr}" | tr '\n' ';')"
  alert_emit critical "external_chisel_check_failed" \
    "External chisel checks failed (relay=${relay_code} kafka=${kafka_result} url=${CHISEL_CHECK_URL} details=${tail_reason})"
  exit 1
}

main "$@"
