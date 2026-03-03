#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

readonly DEFAULT_LOG_DIR="/var/log/firecracker/watchdog"
readonly DEFAULT_FALLBACK_LOG_DIR="/tmp/firecracker-watchdog"
readonly DEFAULT_LOG_FILE_NAME="watchdog.log"
readonly DEFAULT_RELAY_VM_SERVICE="firecracker@relay.service"
readonly DEFAULT_RELAY_VM_IP="172.30.0.20"
readonly DEFAULT_RELAY_VM_PORT="8080"
readonly DEFAULT_RELAY_VM_SOCKET="/tmp/relay-fc.sock"
readonly DEFAULT_RELAY_HEALTH_URL=""
readonly DEFAULT_RELAY_HEALTH_EXPECT_SUBSTRING="ok"
readonly DEFAULT_RELAY_HEALTH_RESTART_SERVICE="firecracker@relay.service"
readonly DEFAULT_REQUIRED_SERVICES="firecracker-network.service,firecracker@relay.service"
readonly DEFAULT_BROKER_INVENTORY_SCRIPT="${SCRIPT_DIR}/../runtime/broker_inventory.sh"
readonly DEFAULT_BROKER_ROW=$'kafka\t1\t172.30.0.10\ttap-kafka\t/tmp/kafka-fc.sock\t9092\t\t'
readonly DEFAULT_BROKER_SERVICE_TEMPLATE="firecracker@%s.service"
readonly DEFAULT_BROKER_SOCKET_TEMPLATE="/tmp/%s-fc.sock"
readonly DEFAULT_BROKER_PORT="9092"
readonly DEFAULT_HEARTBEAT_SCRIPT="${SCRIPT_DIR}/heartbeat.sh"
readonly DEFAULT_ALERT_HELPER_SCRIPT="${SCRIPT_DIR}/alert.sh"
readonly DEFAULT_RESTART_DELAY_SECONDS="5"
readonly DEFAULT_WATCHDOG_ENV_FILE="/etc/firecracker/watchdog.env"

WATCHDOG_ENV_FILE="${FIRECRACKER_WATCHDOG_ENV_FILE:-${DEFAULT_WATCHDOG_ENV_FILE}}"
if [ -f "${WATCHDOG_ENV_FILE}" ]; then
  # shellcheck disable=SC1090
  . "${WATCHDOG_ENV_FILE}"
fi

WATCHDOG_LOG_DIR="${FIRECRACKER_WATCHDOG_LOG_DIR:-${DEFAULT_LOG_DIR}}"
WATCHDOG_FALLBACK_LOG_DIR="${FIRECRACKER_WATCHDOG_FALLBACK_LOG_DIR:-${DEFAULT_FALLBACK_LOG_DIR}}"
WATCHDOG_LOG_FILE_NAME="${FIRECRACKER_WATCHDOG_LOG_FILE_NAME:-${DEFAULT_LOG_FILE_NAME}}"
RELAY_VM_SERVICE="${FIRECRACKER_WATCHDOG_RELAY_SERVICE:-${DEFAULT_RELAY_VM_SERVICE}}"
RELAY_VM_IP="${FIRECRACKER_WATCHDOG_RELAY_VM_IP:-${DEFAULT_RELAY_VM_IP}}"
RELAY_VM_PORT="${FIRECRACKER_WATCHDOG_RELAY_VM_PORT:-${DEFAULT_RELAY_VM_PORT}}"
RELAY_VM_SOCKET="${FIRECRACKER_WATCHDOG_RELAY_SOCKET:-${DEFAULT_RELAY_VM_SOCKET}}"
RELAY_HEALTH_URL="${FIRECRACKER_WATCHDOG_RELAY_HEALTH_URL:-${DEFAULT_RELAY_HEALTH_URL}}"
RELAY_HEALTH_EXPECT_SUBSTRING="${FIRECRACKER_WATCHDOG_RELAY_HEALTH_EXPECT_SUBSTRING:-${DEFAULT_RELAY_HEALTH_EXPECT_SUBSTRING}}"
RELAY_HEALTH_RESTART_SERVICE="${FIRECRACKER_WATCHDOG_RELAY_HEALTH_RESTART_SERVICE:-${DEFAULT_RELAY_HEALTH_RESTART_SERVICE}}"
REQUIRED_SERVICES="${FIRECRACKER_WATCHDOG_REQUIRED_SERVICES:-${DEFAULT_REQUIRED_SERVICES}}"
BROKER_INVENTORY_SCRIPT="${FIRECRACKER_BROKER_INVENTORY_SCRIPT:-${DEFAULT_BROKER_INVENTORY_SCRIPT}}"
BROKER_SERVICE_TEMPLATE="${FIRECRACKER_WATCHDOG_BROKER_SERVICE_TEMPLATE:-${DEFAULT_BROKER_SERVICE_TEMPLATE}}"
BROKER_SOCKET_TEMPLATE="${FIRECRACKER_WATCHDOG_BROKER_SOCKET_TEMPLATE:-${DEFAULT_BROKER_SOCKET_TEMPLATE}}"
BROKER_PORT="${FIRECRACKER_WATCHDOG_BROKER_PORT:-${DEFAULT_BROKER_PORT}}"
HEARTBEAT_SCRIPT="${FIRECRACKER_WATCHDOG_HEARTBEAT_SCRIPT:-${DEFAULT_HEARTBEAT_SCRIPT}}"
ALERT_HELPER_SCRIPT="${FIRECRACKER_WATCHDOG_ALERT_HELPER:-${DEFAULT_ALERT_HELPER_SCRIPT}}"
RESTART_DELAY_SECONDS="${FIRECRACKER_WATCHDOG_RESTART_DELAY_SECONDS:-${DEFAULT_RESTART_DELAY_SECONDS}}"
CHISEL_HEALTH_HOST_PORT="${FIRECRACKER_WATCHDOG_CHISEL_HOST_PORT:-}"
CHISEL_SERVICE_NAME="${FIRECRACKER_WATCHDOG_CHISEL_SERVICE:-}"
ENABLE_KAFKA_METADATA_CHECK="${FIRECRACKER_WATCHDOG_ENABLE_KAFKA_METADATA_CHECK:-false}"

WATCHDOG_LOG_FILE=""
RECOVERED_COUNT=0

cmd_exists() {
  command -v "$1" >/dev/null 2>&1
}

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

resolve_log_dir() {
  local requested_dir="$1"
  local fallback_dir="$2"

  if ensure_writable_dir "${requested_dir}"; then
    printf '%s\n' "${requested_dir}"
    return 0
  fi

  if ensure_writable_dir "${fallback_dir}"; then
    printf '%s\n' "${fallback_dir}"
    return 0
  fi

  return 1
}

log() {
  local now_text=""

  now_text="$(date '+%Y-%m-%d %H:%M:%S')"
  if [ -n "${WATCHDOG_LOG_FILE}" ]; then
    printf '[%s] %s\n' "${now_text}" "$*" | tee -a "${WATCHDOG_LOG_FILE}" >&2
  else
    printf '[%s] %s\n' "${now_text}" "$*" >&2
  fi
}

load_alert_helper() {
  if [ -f "${ALERT_HELPER_SCRIPT}" ]; then
    # shellcheck disable=SC1090
    . "${ALERT_HELPER_SCRIPT}"
  fi

  if ! declare -F alert_emit >/dev/null 2>&1; then
    alert_emit() {
      :
    }
  fi
}

load_broker_inventory() {
  if [ -f "${BROKER_INVENTORY_SCRIPT}" ]; then
    # shellcheck disable=SC1090
    . "${BROKER_INVENTORY_SCRIPT}"
  fi
}

collect_broker_rows() {
  if declare -F inventory_rows >/dev/null 2>&1; then
    mapfile -t broker_rows < <(inventory_rows)
  else
    broker_rows=()
  fi

  if [ "${#broker_rows[@]}" -eq 0 ]; then
    broker_rows=("${DEFAULT_BROKER_ROW}")
  fi

  printf '%s\n' "${broker_rows[@]}"
}

run_diagnostics() {
  {
    printf '=== diagnostics (%s) ===\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    if cmd_exists ip; then
      ip addr show
      ip route
    fi
    if cmd_exists ss; then
      ss -tlnp
    fi
  } >> "${WATCHDOG_LOG_FILE}" 2>&1 || true
}

restart_service() {
  local service_name="$1"

  if [ -z "${service_name}" ]; then
    return 1
  fi

  if ! cmd_exists systemctl; then
    log "warn: systemctl missing; cannot restart ${service_name}"
    return 1
  fi

  log "restart: ${service_name}"
  run_diagnostics
  systemctl restart "${service_name}" || true
  sleep "${RESTART_DELAY_SECONDS}"

  if systemctl is-active --quiet "${service_name}"; then
    log "restart-ok: ${service_name}"
    return 0
  fi

  log "restart-failed: ${service_name}"
  alert_emit critical "service_restart_failed_${service_name}" "failed to recover service ${service_name}"
  return 1
}

find_firecracker_pid() {
  local vm_id="$1"
  local socket_path="$2"
  local process_id=""

  if ! cmd_exists pgrep; then
    return 1
  fi

  process_id="$(pgrep -f "/firecracker --id ${vm_id}" 2>/dev/null | head -n1 || true)"
  if [ -n "${process_id}" ]; then
    printf '%s\n' "${process_id}"
    return 0
  fi

  process_id="$(pgrep -f "firecracker --api-sock ${socket_path}" 2>/dev/null | head -n1 || true)"
  if [ -n "${process_id}" ]; then
    printf '%s\n' "${process_id}"
    return 0
  fi

  return 1
}

check_firecracker_process_state() {
  local vm_id="$1"
  local service_name="$2"
  local socket_path="$3"
  local process_id=""
  local process_state=""

  process_id="$(find_firecracker_pid "${vm_id}" "${socket_path}" || true)"
  [ -n "${process_id}" ] || return 0

  process_state="$(ps -o stat= -p "${process_id}" 2>/dev/null | tr -d ' ' || printf 'unknown')"
  if [[ "${process_state}" =~ D|Z ]]; then
    log "alert: firecracker ${vm_id} pid=${process_id} state=${process_state}; restarting ${service_name}"
    alert_emit critical "firecracker_${vm_id}_stuck" "firecracker ${vm_id} process state=${process_state}; restarting ${service_name}"
    kill -9 "${process_id}" >/dev/null 2>&1 || true
    restart_service "${service_name}" || true
    RECOVERED_COUNT=$((RECOVERED_COUNT + 1))
  fi
}

port_open() {
  local host_ip="$1"
  local port_number="$2"

  if cmd_exists nc; then
    nc -z -w3 "${host_ip}" "${port_number}" >/dev/null 2>&1
    return $?
  fi

  if cmd_exists timeout; then
    timeout 3 bash -lc "</dev/tcp/${host_ip}/${port_number}" >/dev/null 2>&1
    return $?
  fi

  bash -lc "</dev/tcp/${host_ip}/${port_number}" >/dev/null 2>&1
}

check_required_service() {
  local service_name="$1"
  local service_state=""

  [ -n "${service_name}" ] || return 0

  if ! cmd_exists systemctl; then
    return 0
  fi

  service_state="$(systemctl is-active "${service_name}" 2>/dev/null || printf 'unknown')"
  if [ "${service_state}" != "active" ]; then
    log "alert: required service ${service_name} is ${service_state}"
    alert_emit warning "required_service_down_${service_name}" "required service ${service_name} is ${service_state}"
    restart_service "${service_name}" || true
    RECOVERED_COUNT=$((RECOVERED_COUNT + 1))
  fi
}

maybe_run_kafka_metadata_check() {
  local endpoint="$1"

  if ! as_bool "${ENABLE_KAFKA_METADATA_CHECK}"; then
    return 0
  fi

  if ! cmd_exists kcat; then
    log "warn: kcat missing; skipping metadata check"
    return 0
  fi

  if ! cmd_exists timeout; then
    log "warn: timeout missing; skipping metadata check"
    return 0
  fi

  if ! timeout 10 kcat -b "${endpoint}" -L >/dev/null 2>&1; then
    log "alert: kafka metadata check failed at ${endpoint}"
    alert_emit warning "kafka_metadata_failed" "kafka metadata check failed at ${endpoint}"
  fi
}

main() {
  local log_dir=""
  local broker_row=""
  local broker_id=""
  local broker_ip=""
  local broker_socket=""
  local host_proxy_port=""
  local broker_service_name=""
  local required_service=""
  local relay_health_body=""
  local relay_health_url_ok=0
  local primary_broker_endpoint=""
  local chisel_host=""
  local chisel_port=""

  log_dir="$(resolve_log_dir "${WATCHDOG_LOG_DIR}" "${WATCHDOG_FALLBACK_LOG_DIR}" || true)"
  if [ -z "${log_dir}" ]; then
    printf 'error: unable to write watchdog logs in %s or %s\n' "${WATCHDOG_LOG_DIR}" "${WATCHDOG_FALLBACK_LOG_DIR}" >&2
    exit 1
  fi

  WATCHDOG_LOG_FILE="${log_dir}/${WATCHDOG_LOG_FILE_NAME}"
  touch "${WATCHDOG_LOG_FILE}" >/dev/null 2>&1 || {
    printf 'error: unable to write watchdog log file: %s\n' "${WATCHDOG_LOG_FILE}" >&2
    exit 1
  }

  load_alert_helper
  load_broker_inventory

  mapfile -t broker_rows < <(collect_broker_rows)

  for broker_row in "${broker_rows[@]}"; do
    IFS=$'\t' read -r broker_id _ broker_ip _ broker_socket host_proxy_port _ _ <<<"${broker_row}"
    [ -n "${broker_id}" ] || continue
    [ -n "${broker_ip}" ] || continue
    [ -n "${broker_socket}" ] || broker_socket="$(printf "${BROKER_SOCKET_TEMPLATE}" "${broker_id}")"

    broker_service_name="$(printf "${BROKER_SERVICE_TEMPLATE}" "${broker_id}")"
    check_firecracker_process_state "${broker_id}" "${broker_service_name}" "${broker_socket}"

    if ! port_open "${broker_ip}" "${BROKER_PORT}"; then
      log "alert: broker ${broker_id} ${broker_ip}:${BROKER_PORT} down"
      alert_emit critical "kafka_port_down_${broker_id}" "broker ${broker_id} at ${broker_ip}:${BROKER_PORT} is down"
      restart_service "${broker_service_name}" || true
      RECOVERED_COUNT=$((RECOVERED_COUNT + 1))
    fi

    if [ -z "${primary_broker_endpoint}" ]; then
      if [[ "${host_proxy_port:-}" =~ ^[0-9]+$ ]] && [ "${host_proxy_port}" -gt 0 ]; then
        primary_broker_endpoint="127.0.0.1:${host_proxy_port}"
      else
        primary_broker_endpoint="${broker_ip}:${BROKER_PORT}"
      fi
    fi
  done

  check_firecracker_process_state "relay" "${RELAY_VM_SERVICE}" "${RELAY_VM_SOCKET}"

  if ! port_open "${RELAY_VM_IP}" "${RELAY_VM_PORT}"; then
    log "alert: relay ${RELAY_VM_IP}:${RELAY_VM_PORT} down"
    alert_emit critical "relay_port_down" "relay port ${RELAY_VM_IP}:${RELAY_VM_PORT} is down"
    restart_service "${RELAY_VM_SERVICE}" || true
    RECOVERED_COUNT=$((RECOVERED_COUNT + 1))
  fi

  if [ -n "${RELAY_HEALTH_URL}" ]; then
    relay_health_body="$(curl -s --max-time 5 "${RELAY_HEALTH_URL}" 2>/dev/null || true)"
    if [ -n "${relay_health_body}" ]; then
      if [ -z "${RELAY_HEALTH_EXPECT_SUBSTRING}" ] || grep -q "${RELAY_HEALTH_EXPECT_SUBSTRING}" <<<"${relay_health_body}"; then
        relay_health_url_ok=1
      fi
    fi

    if [ "${relay_health_url_ok}" -ne 1 ]; then
      log "alert: relay health check failed at ${RELAY_HEALTH_URL}"
      alert_emit warning "relay_health_failed" "relay health check failed at ${RELAY_HEALTH_URL}"
      if [ -n "${RELAY_HEALTH_RESTART_SERVICE}" ]; then
        restart_service "${RELAY_HEALTH_RESTART_SERVICE}" || true
        RECOVERED_COUNT=$((RECOVERED_COUNT + 1))
      fi
    fi
  fi

  if [ -n "${CHISEL_HEALTH_HOST_PORT}" ]; then
    chisel_host="${CHISEL_HEALTH_HOST_PORT%%:*}"
    chisel_port="${CHISEL_HEALTH_HOST_PORT##*:}"
    if ! port_open "${chisel_host}" "${chisel_port}"; then
      log "alert: chisel endpoint ${CHISEL_HEALTH_HOST_PORT} down"
      alert_emit critical "chisel_down" "chisel endpoint ${CHISEL_HEALTH_HOST_PORT} is unavailable"
      if [ -n "${CHISEL_SERVICE_NAME}" ]; then
        restart_service "${CHISEL_SERVICE_NAME}" || true
        RECOVERED_COUNT=$((RECOVERED_COUNT + 1))
      fi
    fi
  fi

  IFS=',' read -r -a required_services_array <<< "${REQUIRED_SERVICES}"
  for required_service in "${required_services_array[@]}"; do
    required_service="$(printf '%s' "${required_service}" | xargs)"
    check_required_service "${required_service}"
  done

  if [ -n "${primary_broker_endpoint}" ]; then
    maybe_run_kafka_metadata_check "${primary_broker_endpoint}"
  fi

  if [ -x "${HEARTBEAT_SCRIPT}" ]; then
    "${HEARTBEAT_SCRIPT}" || {
      log "alert: heartbeat script failed: ${HEARTBEAT_SCRIPT}"
      alert_emit warning "heartbeat_failed" "heartbeat script failed: ${HEARTBEAT_SCRIPT}"
    }
  else
    log "warn: heartbeat script missing or not executable: ${HEARTBEAT_SCRIPT}"
  fi

  if [ "${RECOVERED_COUNT}" -gt 0 ]; then
    log "watchdog: recovered ${RECOVERED_COUNT} services"
    alert_emit warning "watchdog_recovered" "watchdog recovered ${RECOVERED_COUNT} services"
  else
    log "watchdog: all checks healthy"
  fi
}

main "$@"
