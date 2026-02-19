#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

# Linear webhook relay:
# - Verify HMAC signature (Linear-Signature)
# - Skip agent-authored events to avoid feedback loops
# - Deduplicate deliveries for replay protection
# - Apply short per-entity cooldown
# - Sanitize payload before forwarding to OpenClaw

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
SANITIZER_PATH="${SCRIPT_DIR}/sanitize-payload.py"
readonly SANITIZER_PATH

readonly DEFAULT_DEDUP_RETENTION_DAYS=7
readonly DEFAULT_COOLDOWN_SECONDS=30
readonly DEFAULT_TIMESTAMP_WINDOW_SECONDS=60
readonly DEFAULT_CURL_CONNECT_TIMEOUT_SECONDS=5
readonly DEFAULT_CURL_MAX_TIME_SECONDS=20

readonly DEDUP_DIR="${WEBHOOK_DEDUP_DIR:-/tmp/webhook-dedup}"
readonly DEDUP_RETENTION_DAYS="${WEBHOOK_DEDUP_RETENTION_DAYS:-$DEFAULT_DEDUP_RETENTION_DAYS}"
readonly LINEAR_EVENT_COOLDOWN_SECONDS="${LINEAR_COOLDOWN_SECONDS:-$DEFAULT_COOLDOWN_SECONDS}"
readonly LINEAR_TIMESTAMP_WINDOW_SECONDS="${LINEAR_TIMESTAMP_WINDOW_SECONDS:-$DEFAULT_TIMESTAMP_WINDOW_SECONDS}"
readonly LINEAR_ENFORCE_TIMESTAMP_CHECK="${LINEAR_ENFORCE_TIMESTAMP_CHECK:-true}"
readonly CURL_CONNECT_TIMEOUT_SECONDS="${WEBHOOK_CURL_CONNECT_TIMEOUT_SECONDS:-$DEFAULT_CURL_CONNECT_TIMEOUT_SECONDS}"
readonly CURL_MAX_TIME_SECONDS="${WEBHOOK_CURL_MAX_TIME_SECONDS:-$DEFAULT_CURL_MAX_TIME_SECONDS}"

log() {
  printf '%s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

require_env() {
  local name="$1"
  if [ -z "${!name:-}" ]; then
    die "missing required environment variable: ${name}"
  fi
}

hash_sha256() {
  local value="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    printf '%s' "${value}" | sha256sum | awk '{print $1}'
    return
  fi
  if command -v shasum >/dev/null 2>&1; then
    printf '%s' "${value}" | shasum -a 256 | awk '{print $1}'
    return
  fi
  if command -v openssl >/dev/null 2>&1; then
    printf '%s' "${value}" | openssl dgst -sha256 | awk '{print $2}'
    return
  fi
  die "no SHA-256 tool available (need sha256sum, shasum, or openssl)"
}

file_mtime_epoch() {
  local path="$1"
  if stat -f '%m' "${path}" >/dev/null 2>&1; then
    stat -f '%m' "${path}"
    return
  fi
  stat -c '%Y' "${path}"
}

to_lower() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]'
}

normalize_signature() {
  local signature="$1"
  signature="${signature#sha256=}"
  signature="$(printf '%s' "${signature}" | tr -d '[:space:]')"
  to_lower "${signature}"
}

constant_time_equals() {
  local left="$1"
  local right="$2"
  python3 - "$left" "$right" <<'PY'
import hmac
import sys

if len(sys.argv) != 3:
    raise SystemExit(2)

raise SystemExit(0 if hmac.compare_digest(sys.argv[1], sys.argv[2]) else 1)
PY
}

is_truthy() {
  case "$1" in
    1|true|TRUE|yes|YES|on|ON) return 0 ;;
    *) return 1 ;;
  esac
}

cleanup_old_dedup_entries() {
  find "${DEDUP_DIR}" -type f -mtime +"${DEDUP_RETENTION_DAYS}" -delete 2>/dev/null || true
}

resolve_linear_entity_id() {
  if [ -n "${LINEAR_ENTITY_ID:-}" ]; then
    printf '%s' "${LINEAR_ENTITY_ID}"
    return
  fi
  printf '%s' "${LINEAR_PAYLOAD}" | python3 -c '
import json, sys
try:
    payload = json.load(sys.stdin)
    print((payload.get("data") or {}).get("id") or "", end="")
except Exception:
    print("", end="")
'
}

resolve_linear_team_key() {
  if [ -n "${LINEAR_TEAM_KEY:-}" ]; then
    printf '%s' "${LINEAR_TEAM_KEY}"
    return
  fi
  printf '%s' "${LINEAR_PAYLOAD}" | python3 -c '
import json, sys
try:
    payload = json.load(sys.stdin)
    team = (payload.get("data") or {}).get("team") or {}
    print(team.get("key") or "unknown", end="")
except Exception:
    print("unknown", end="")
'
}

extract_linear_webhook_timestamp_epoch() {
  printf '%s' "${LINEAR_PAYLOAD}" | python3 -c '
import json, sys
try:
    payload = json.load(sys.stdin)
    timestamp = payload.get("webhookTimestamp")
    if timestamp is None:
        print("", end="")
        raise SystemExit(0)

    value = int(timestamp)
    if value > 10_000_000_000:
        value //= 1000
    print(value, end="")
except Exception:
    print("", end="")
'
}

mark_and_check_duplicate() {
  local dedup_key="$1"
  local dedup_file
  dedup_file="${DEDUP_DIR}/$(hash_sha256 "${dedup_key}")"

  mkdir -p "${DEDUP_DIR}"
  if [ -f "${dedup_file}" ]; then
    log "duplicate delivery skipped: ${dedup_key}"
    return 0
  fi

  : > "${dedup_file}"
  cleanup_old_dedup_entries
  return 1
}

verify_linear_timestamp_window() {
  if ! is_truthy "${LINEAR_ENFORCE_TIMESTAMP_CHECK}"; then
    return
  fi

  local webhook_timestamp_epoch
  webhook_timestamp_epoch="$(extract_linear_webhook_timestamp_epoch)"
  if [ -z "${webhook_timestamp_epoch}" ]; then
    die "missing or invalid webhookTimestamp (set LINEAR_ENFORCE_TIMESTAMP_CHECK=false to bypass)"
  fi

  local now_epoch
  local skew_seconds
  now_epoch="$(date +%s)"
  skew_seconds=$((now_epoch - webhook_timestamp_epoch))
  if [ "${skew_seconds}" -lt 0 ]; then
    skew_seconds=$((skew_seconds * -1))
  fi

  if [ "${skew_seconds}" -gt "${LINEAR_TIMESTAMP_WINDOW_SECONDS}" ]; then
    die "stale Linear webhook timestamp (skew ${skew_seconds}s > ${LINEAR_TIMESTAMP_WINDOW_SECONDS}s)"
  fi
}

check_cooldown() {
  local team_key="$1"
  local entity_id="$2"
  local cooldown_file="${DEDUP_DIR}/cooldown-linear-${team_key}-${entity_id}"

  if [ ! -f "${cooldown_file}" ]; then
    : > "${cooldown_file}"
    return 1
  fi

  local now_epoch
  local file_epoch
  local age_seconds
  now_epoch="$(date +%s)"
  file_epoch="$(file_mtime_epoch "${cooldown_file}")"
  age_seconds=$((now_epoch - file_epoch))

  if [ "${age_seconds}" -lt "${LINEAR_EVENT_COOLDOWN_SECONDS}" ]; then
    log "cooldown active (${age_seconds}s < ${LINEAR_EVENT_COOLDOWN_SECONDS}s) for ${team_key}:${entity_id}"
    return 0
  fi

  : > "${cooldown_file}"
  return 1
}

verify_linear_signature() {
  require_env "LINEAR_WEBHOOK_SECRET"
  require_env "LINEAR_SIGNATURE"
  require_env "LINEAR_PAYLOAD"

  local expected_signature
  expected_signature="$(printf '%s' "${LINEAR_PAYLOAD}" | openssl dgst -sha256 -hmac "${LINEAR_WEBHOOK_SECRET}" | awk '{print $NF}')"

  local provided_signature
  provided_signature="$(normalize_signature "${LINEAR_SIGNATURE}")"
  expected_signature="$(normalize_signature "${expected_signature}")"

  if ! constant_time_equals "${provided_signature}" "${expected_signature}"; then
    die "invalid Linear signature"
  fi
}

sanitize_payload() {
  if [ ! -f "${SANITIZER_PATH}" ]; then
    die "missing sanitizer script: ${SANITIZER_PATH}"
  fi
  printf '%s' "${LINEAR_PAYLOAD}" | python3 "${SANITIZER_PATH}" --source linear --verbose
}

forward_to_openclaw() {
  local sanitized_payload="$1"
  local openclaw_url="${OPENCLAW_GATEWAY_URL%/}/hooks/agent?source=linear"

  curl --silent --show-error --fail \
    --connect-timeout "${CURL_CONNECT_TIMEOUT_SECONDS}" \
    --max-time "${CURL_MAX_TIME_SECONDS}" \
    -X POST "${openclaw_url}" \
    -H "Authorization: Bearer ${OPENCLAW_HOOKS_TOKEN}" \
    -H "Content-Type: application/json" \
    -H "X-Webhook-Source: linear" \
    -H "X-Linear-Event: ${LINEAR_EVENT_TYPE:-}" \
    -H "X-Linear-Delivery: ${LINEAR_DELIVERY}" \
    -d "${sanitized_payload}" >/dev/null
}

main() {
  require_env "OPENCLAW_GATEWAY_URL"
  require_env "OPENCLAW_HOOKS_TOKEN"
  require_env "LINEAR_DELIVERY"
  require_env "LINEAR_ACTION"
  require_env "LINEAR_PAYLOAD"

  verify_linear_signature
  verify_linear_timestamp_window

  if [ -n "${LINEAR_AGENT_USER_ID:-}" ] && [ -n "${LINEAR_ACTOR_ID:-}" ] && [ "${LINEAR_AGENT_USER_ID}" = "${LINEAR_ACTOR_ID}" ]; then
    log "skipping event from agent user: ${LINEAR_ACTOR_ID}"
    exit 0
  fi

  local entity_id
  local team_key
  entity_id="$(resolve_linear_entity_id)"
  team_key="$(resolve_linear_team_key)"
  if [ -z "${entity_id}" ]; then
    entity_id="unknown"
  fi

  local dedup_key="linear:${LINEAR_DELIVERY}:${LINEAR_ACTION}:${entity_id}"
  if mark_and_check_duplicate "${dedup_key}"; then
    exit 0
  fi

  if check_cooldown "${team_key}" "${entity_id}"; then
    exit 0
  fi

  local sanitized_payload
  sanitized_payload="$(sanitize_payload)"
  forward_to_openclaw "${sanitized_payload}"
  log "relay complete: linear ${LINEAR_EVENT_TYPE:-unknown} ${team_key}:${entity_id}"
}

main "$@"
