#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

# GitHub webhook relay:
# - Skip bot-generated events to avoid feedback loops
# - Deduplicate deliveries for replay protection
# - Apply a short per-PR cooldown to avoid event storms
# - Sanitize payload before forwarding to OpenClaw

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly SCRIPT_DIR
SANITIZER_PATH="${SCRIPT_DIR}/sanitize-payload.py"
readonly SANITIZER_PATH

readonly DEFAULT_DEDUP_RETENTION_DAYS=7
readonly DEFAULT_COOLDOWN_SECONDS=30
readonly DEFAULT_CURL_CONNECT_TIMEOUT_SECONDS=5
readonly DEFAULT_CURL_MAX_TIME_SECONDS=20

readonly DEDUP_DIR="${WEBHOOK_DEDUP_DIR:-/tmp/webhook-dedup}"
readonly DEDUP_RETENTION_DAYS="${WEBHOOK_DEDUP_RETENTION_DAYS:-$DEFAULT_DEDUP_RETENTION_DAYS}"
readonly GITHUB_EVENT_COOLDOWN_SECONDS="${GITHUB_COOLDOWN_SECONDS:-$DEFAULT_COOLDOWN_SECONDS}"
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

cleanup_old_dedup_entries() {
  find "${DEDUP_DIR}" -type f -mtime +"${DEDUP_RETENTION_DAYS}" -delete 2>/dev/null || true
}

resolve_github_sender() {
  if [ -n "${GITHUB_SENDER:-}" ]; then
    printf '%s' "${GITHUB_SENDER}"
    return
  fi
  printf '%s' "${GITHUB_PAYLOAD}" | python3 -c '
import json, sys
try:
    payload = json.load(sys.stdin)
    print((payload.get("sender") or {}).get("login") or "", end="")
except Exception:
    print("", end="")
'
}

resolve_github_entity_id() {
  if [ -n "${GITHUB_PR_NUMBER:-}" ]; then
    printf '%s' "${GITHUB_PR_NUMBER}"
    return
  fi
  printf '%s' "${GITHUB_PAYLOAD}" | python3 -c '
import json, sys
try:
    payload = json.load(sys.stdin)
    pr_number = (payload.get("pull_request") or {}).get("number")
    issue_number = (payload.get("issue") or {}).get("number")
    fallback = payload.get("number")
    value = pr_number or issue_number or fallback or ""
    print(value, end="")
except Exception:
    print("", end="")
'
}

is_bot_sender() {
  local sender="$1"
  case "${sender}" in
    *"[bot]") return 0 ;;
    *) return 1 ;;
  esac
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

check_cooldown() {
  local repo="$1"
  local entity_id="$2"
  local cooldown_file="${DEDUP_DIR}/cooldown-github-${repo//\//-}-${entity_id}"

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

  if [ "${age_seconds}" -lt "${GITHUB_EVENT_COOLDOWN_SECONDS}" ]; then
    log "cooldown active (${age_seconds}s < ${GITHUB_EVENT_COOLDOWN_SECONDS}s) for ${repo}#${entity_id}"
    return 0
  fi

  : > "${cooldown_file}"
  return 1
}

sanitize_payload() {
  if [ ! -f "${SANITIZER_PATH}" ]; then
    die "missing sanitizer script: ${SANITIZER_PATH}"
  fi
  printf '%s' "${GITHUB_PAYLOAD}" | python3 "${SANITIZER_PATH}" --source github --verbose
}

forward_to_openclaw() {
  local sanitized_payload="$1"
  local openclaw_url="${OPENCLAW_GATEWAY_URL%/}/hooks/agent?source=github-pr"

  curl --silent --show-error --fail \
    --connect-timeout "${CURL_CONNECT_TIMEOUT_SECONDS}" \
    --max-time "${CURL_MAX_TIME_SECONDS}" \
    -X POST "${openclaw_url}" \
    -H "Authorization: Bearer ${OPENCLAW_HOOKS_TOKEN}" \
    -H "Content-Type: application/json" \
    -H "X-Webhook-Source: github" \
    -H "X-GitHub-Event: ${GITHUB_EVENT}" \
    -H "X-GitHub-Delivery: ${GITHUB_DELIVERY}" \
    -H "X-GitHub-Installation: ${GITHUB_INSTALLATION_ID:-}" \
    -d "${sanitized_payload}" >/dev/null
}

main() {
  require_env "OPENCLAW_GATEWAY_URL"
  require_env "OPENCLAW_HOOKS_TOKEN"
  require_env "GITHUB_EVENT"
  require_env "GITHUB_DELIVERY"
  require_env "GITHUB_ACTION"
  require_env "GITHUB_REPO"
  require_env "GITHUB_PAYLOAD"

  local sender_login
  sender_login="$(resolve_github_sender)"
  if is_bot_sender "${sender_login}"; then
    log "skipping bot sender: ${sender_login}"
    exit 0
  fi

  local entity_id
  entity_id="$(resolve_github_entity_id)"
  if [ -z "${entity_id}" ]; then
    entity_id="unknown"
  fi

  local dedup_key="github:${GITHUB_DELIVERY}:${GITHUB_ACTION}:${entity_id}"
  if mark_and_check_duplicate "${dedup_key}"; then
    exit 0
  fi

  if check_cooldown "${GITHUB_REPO}" "${entity_id}"; then
    exit 0
  fi

  local sanitized_payload
  sanitized_payload="$(sanitize_payload)"
  forward_to_openclaw "${sanitized_payload}"
  log "relay complete: github ${GITHUB_EVENT} ${GITHUB_REPO}#${entity_id}"
}

main "$@"
