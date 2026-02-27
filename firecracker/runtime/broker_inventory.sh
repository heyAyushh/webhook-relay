#!/usr/bin/env bash
set -euo pipefail
IFS=$'\n\t'

readonly DEFAULT_BROKER_INVENTORY_PATH="/etc/firecracker/brokers.json"
readonly DEFAULT_BROKER_ROW="kafka\t1\t172.30.0.10\ttap-kafka\t/tmp/kafka-fc.sock\t9092\t/opt/firecracker/kafka/config.json\t/opt/firecracker/kafka/rootfs.ext4"

FC_BROKER_INVENTORY="${FC_BROKER_INVENTORY:-${DEFAULT_BROKER_INVENTORY_PATH}}"

inventory_path() {
  printf '%s\n' "${FC_BROKER_INVENTORY}"
}

inventory_valid() {
  [ -f "${FC_BROKER_INVENTORY}" ] && \
    jq -e '.brokers | type == "array"' "${FC_BROKER_INVENTORY}" >/dev/null 2>&1
}

inventory_rows() {
  if inventory_valid; then
    jq -r '.brokers[] | [.id, .node_id, .ip, .tap, .socket, .host_proxy_port, .config, .rootfs] | @tsv' "${FC_BROKER_INVENTORY}"
    return 0
  fi

  printf '%s\n' "${DEFAULT_BROKER_ROW}"
}

inventory_primary_row() {
  inventory_rows | head -n 1
}

inventory_ids() {
  inventory_rows | cut -f1
}

inventory_taps() {
  inventory_rows | cut -f4
}
