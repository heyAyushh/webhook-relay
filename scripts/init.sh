#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
readonly ENV_TEMPLATE="${REPO_ROOT}/.env.default"
readonly ENV_FILE="${REPO_ROOT}/.env"
readonly CERTS_DIR="${REPO_ROOT}/certs"
readonly TLS_CERT_FILE="${CERTS_DIR}/tls.crt"
readonly TLS_KEY_FILE="${CERTS_DIR}/tls.key"
readonly DEPLOY_ENV_DIR="${REPO_ROOT}/deploy/env"

START_STACK=0

log() {
  printf '%s\n' "$*"
}

usage() {
  cat <<'EOF_USAGE'
Usage: scripts/init.sh [--up]

Options:
  --up    Start docker compose stack after bootstrap
  -h      Show help
EOF_USAGE
}

require_cmd() {
  local cmd="$1"
  command -v "${cmd}" >/dev/null 2>&1 || {
    log "error: missing required command: ${cmd}"
    exit 1
  }
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --up) START_STACK=1 ;;
      -h|--help) usage; exit 0 ;;
      *)
        log "error: unknown option: $1"
        usage
        exit 1
        ;;
    esac
    shift
  done
}

env_value() {
  local key="$1"
  local line
  line="$(grep -E "^${key}=" "${ENV_FILE}" | tail -n1 || true)"
  if [ -z "${line}" ]; then
    printf ''
    return
  fi
  printf '%s' "${line#*=}"
}

upsert_env() {
  local key="$1"
  local value="$2"
  local temp_file

  temp_file="${ENV_FILE}.tmp"
  awk -v target_key="${key}" -v replacement_value="${value}" '
    BEGIN { updated = 0 }
    $0 ~ "^" target_key "=" {
      print target_key "=" replacement_value
      updated = 1
      next
    }
    { print }
    END {
      if (!updated) {
        print target_key "=" replacement_value
      }
    }
  ' "${ENV_FILE}" > "${temp_file}"
  mv "${temp_file}" "${ENV_FILE}"
}

ensure_env_file() {
  if [ ! -f "${ENV_FILE}" ]; then
    cp "${ENV_TEMPLATE}" "${ENV_FILE}"
    log "created ${ENV_FILE} from template"
  else
    log "using existing ${ENV_FILE}"
  fi
}

ensure_secret() {
  local key="$1"
  local current
  current="$(env_value "${key}")"

  if [ -z "${current}" ] || [[ "${current}" == replace-with-* ]]; then
    upsert_env "${key}" "$(openssl rand -hex 32)"
    log "generated secret for ${key}"
  fi
}

ensure_default() {
  local key="$1"
  local value="$2"
  local current
  current="$(env_value "${key}")"

  if [ -z "${current}" ]; then
    upsert_env "${key}" "${value}"
    log "set default ${key}=${value}"
  fi
}

ensure_relay_certs() {
  if [ ! -f "${CERTS_DIR}/ca.crt" ] || [ ! -f "${CERTS_DIR}/relay.crt" ] || [ ! -f "${CERTS_DIR}/consumer.crt" ]; then
    "${REPO_ROOT}/scripts/gen-certs.sh" "${CERTS_DIR}"
  else
    log "existing mTLS certs found in ${CERTS_DIR}"
  fi
}

ensure_nginx_tls_cert() {
  if [ -f "${TLS_CERT_FILE}" ] && [ -f "${TLS_KEY_FILE}" ]; then
    log "existing nginx TLS cert found in ${CERTS_DIR}"
    return
  fi

  mkdir -p "${CERTS_DIR}"
  openssl req -x509 -newkey rsa:2048 -sha256 -nodes \
    -days 825 \
    -subj "/CN=localhost" \
    -keyout "${TLS_KEY_FILE}" \
    -out "${TLS_CERT_FILE}" >/dev/null 2>&1

  chmod 600 "${TLS_KEY_FILE}"
  chmod 644 "${TLS_CERT_FILE}"
  log "generated local nginx TLS cert (${TLS_CERT_FILE})"
}

write_systemd_env_files() {
  mkdir -p "${DEPLOY_ENV_DIR}"

  cat > "${DEPLOY_ENV_DIR}/relay.env" <<EOF_RELAY
RELAY_BIND=$(env_value "RELAY_BIND")
RELAY_MAX_PAYLOAD_BYTES=$(env_value "RELAY_MAX_PAYLOAD_BYTES")
RELAY_IP_RATE_PER_MINUTE=$(env_value "RELAY_IP_RATE_PER_MINUTE")
RELAY_SOURCE_RATE_PER_MINUTE=$(env_value "RELAY_SOURCE_RATE_PER_MINUTE")
RELAY_PUBLISH_QUEUE_CAPACITY=$(env_value "RELAY_PUBLISH_QUEUE_CAPACITY")
RELAY_PUBLISH_MAX_RETRIES=$(env_value "RELAY_PUBLISH_MAX_RETRIES")
RELAY_PUBLISH_BACKOFF_BASE_MS=$(env_value "RELAY_PUBLISH_BACKOFF_BASE_MS")
RELAY_PUBLISH_BACKOFF_MAX_MS=$(env_value "RELAY_PUBLISH_BACKOFF_MAX_MS")
KAFKA_BROKERS=$(env_value "KAFKA_BROKERS")
KAFKA_TLS_CERT=/etc/relay/certs/relay.crt
KAFKA_TLS_KEY=/etc/relay/certs/relay.key
KAFKA_TLS_CA=/etc/relay/certs/ca.crt
KAFKA_AUTO_CREATE_TOPICS=$(env_value "KAFKA_AUTO_CREATE_TOPICS")
KAFKA_TOPIC_PARTITIONS=$(env_value "KAFKA_TOPIC_PARTITIONS")
KAFKA_TOPIC_REPLICATION_FACTOR=$(env_value "KAFKA_TOPIC_REPLICATION_FACTOR")
KAFKA_DLQ_TOPIC=$(env_value "KAFKA_DLQ_TOPIC")
HMAC_SECRET_GITHUB=$(env_value "HMAC_SECRET_GITHUB")
HMAC_SECRET_LINEAR=$(env_value "HMAC_SECRET_LINEAR")
RUST_LOG=$(env_value "RUST_LOG")
EOF_RELAY

  cat > "${DEPLOY_ENV_DIR}/consumer.env" <<EOF_CONSUMER
KAFKA_BROKERS=$(env_value "KAFKA_BROKERS")
KAFKA_TLS_CERT=/etc/consumer/certs/consumer.crt
KAFKA_TLS_KEY=/etc/consumer/certs/consumer.key
KAFKA_TLS_CA=/etc/consumer/certs/ca.crt
KAFKA_GROUP_ID=$(env_value "KAFKA_GROUP_ID")
KAFKA_TOPICS=$(env_value "KAFKA_TOPICS")
KAFKA_DLQ_TOPIC=$(env_value "KAFKA_DLQ_TOPIC")
OPENCLAW_WEBHOOK_URL=$(env_value "OPENCLAW_WEBHOOK_URL")
OPENCLAW_WEBHOOK_TOKEN=$(env_value "OPENCLAW_WEBHOOK_TOKEN")
CONSUMER_MAX_RETRIES=$(env_value "CONSUMER_MAX_RETRIES")
CONSUMER_BACKOFF_BASE_SECONDS=$(env_value "CONSUMER_BACKOFF_BASE_SECONDS")
CONSUMER_BACKOFF_MAX_SECONDS=$(env_value "CONSUMER_BACKOFF_MAX_SECONDS")
RUST_LOG=$(env_value "RUST_LOG")
EOF_CONSUMER

  log "wrote ${DEPLOY_ENV_DIR}/relay.env"
  log "wrote ${DEPLOY_ENV_DIR}/consumer.env"
}

start_stack_if_requested() {
  if [ "${START_STACK}" -ne 1 ]; then
    return
  fi

  require_cmd docker
  (cd "${REPO_ROOT}" && docker compose up --build -d)
  log "docker compose stack started"
}

main() {
  parse_args "$@"

  require_cmd openssl
  require_cmd awk

  ensure_env_file

  ensure_default "RELAY_BIND" "0.0.0.0:8080"
  ensure_default "RELAY_MAX_PAYLOAD_BYTES" "1048576"
  ensure_default "RELAY_IP_RATE_PER_MINUTE" "100"
  ensure_default "RELAY_SOURCE_RATE_PER_MINUTE" "500"
  ensure_default "KAFKA_AUTO_CREATE_TOPICS" "true"
  ensure_default "KAFKA_TOPIC_PARTITIONS" "3"
  ensure_default "KAFKA_TOPIC_REPLICATION_FACTOR" "1"
  ensure_default "KAFKA_DLQ_TOPIC" "webhooks.dlq"
  ensure_default "KAFKA_GROUP_ID" "openclaw-consumer"
  ensure_default "KAFKA_TOPICS" "webhooks.github,webhooks.linear"
  ensure_default "CONSUMER_MAX_RETRIES" "5"
  ensure_default "CONSUMER_BACKOFF_BASE_SECONDS" "1"
  ensure_default "CONSUMER_BACKOFF_MAX_SECONDS" "30"
  ensure_default "RUST_LOG" "info"
  ensure_default "OPENCLAW_WEBHOOK_URL" "http://127.0.0.1:18789/hooks/agent"

  ensure_secret "HMAC_SECRET_GITHUB"
  ensure_secret "HMAC_SECRET_LINEAR"
  ensure_secret "OPENCLAW_WEBHOOK_TOKEN"

  ensure_relay_certs
  ensure_nginx_tls_cert
  write_systemd_env_files
  start_stack_if_requested

  log "init complete"
  log "next: review ${ENV_FILE}, then run scripts/init.sh --up (or docker compose up --build)"
}

main "$@"
