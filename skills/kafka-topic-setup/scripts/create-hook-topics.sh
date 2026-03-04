#!/usr/bin/env bash
# Create the standard hook Kafka topic set.
# Run on the Kafka guest or any host that can reach the broker.
#
# Optional env vars:
#   KAFKA_BOOTSTRAP  - broker address (default 127.0.0.1:9092)
#   SOURCES          - space-separated source names (default "github linear")
#   PARTITIONS       - partition count for source/core topics (default 3)
#   REPLICATION      - replication factor (default 1)
#   RETENTION_MS     - retention for source/core topics in ms (default 7 days)
#   DLQ_RETENTION_MS - retention for DLQ in ms (default 30 days)
set -euo pipefail
IFS=$'\n\t'

KAFKA_BOOTSTRAP="${KAFKA_BOOTSTRAP:-127.0.0.1:9092}"
SOURCES="${SOURCES:-github linear}"
PARTITIONS="${PARTITIONS:-3}"
REPLICATION="${REPLICATION:-1}"
RETENTION_MS="${RETENTION_MS:-604800000}"
DLQ_RETENTION_MS="${DLQ_RETENTION_MS:-2592000000}"
KAFKA_BIN="${KAFKA_BIN:-/opt/kafka/bin}"

log() { printf '%s\n' "$*" >&2; }

create_topic() {
  local topic="$1"
  local partitions="$2"
  local extra_config="${3:-}"

  local args=(
    --bootstrap-server "${KAFKA_BOOTSTRAP}"
    --create --if-not-exists
    --topic "${topic}"
    --partitions "${partitions}"
    --replication-factor "${REPLICATION}"
    --config "retention.ms=${RETENTION_MS}"
    --config "cleanup.policy=delete"
  )

  if [ -n "${extra_config}" ]; then
    args+=(--config "${extra_config}")
  fi

  log "creating topic: ${topic} (partitions=${partitions})"
  "${KAFKA_BIN}/kafka-topics.sh" "${args[@]}"
}

main() {
  log "bootstrap: ${KAFKA_BOOTSTRAP}"
  log "sources: ${SOURCES}"

  for source in ${SOURCES}; do
    create_topic "webhooks.${source}" "${PARTITIONS}"
  done

  create_topic "webhooks.core" "${PARTITIONS}"
  create_topic "webhooks.dlq" "1" "retention.ms=${DLQ_RETENTION_MS}"

  log ""
  log "topics created. listing webhooks.*:"
  "${KAFKA_BIN}/kafka-topics.sh" \
    --bootstrap-server "${KAFKA_BOOTSTRAP}" \
    --list | grep "^webhooks\." || true
}

main "$@"
