#!/usr/bin/env bash
# Bootstrap a Kafka broker-only KRaft node that joins an existing cluster.
# Run as root inside the new guest VM.
#
# Required env vars:
#   KAFKA_NODE_ID       - unique node ID (must not conflict with existing nodes)
#   KAFKA_CLUSTER_ID    - cluster ID from the existing controller's meta.properties
#   KAFKA_QUORUM_VOTERS - controller quorum, e.g. "1@172.16.40.2:9093"
#
# Optional env vars (with defaults):
#   KAFKA_VERSION, KAFKA_SCALA_VERSION, KAFKA_ADVERTISED_HOST,
#   KAFKA_BROKER_PORT, KAFKA_LISTEN_ADDRESS, KAFKA_USER, KAFKA_GROUP,
#   KAFKA_INSTALL_DIR, KAFKA_CONFIG_DIR, KAFKA_DATA_DIR
set -euo pipefail
IFS=$'\n\t'

KAFKA_VERSION="${KAFKA_VERSION:-4.0.0}"
KAFKA_SCALA_VERSION="${KAFKA_SCALA_VERSION:-2.13}"
KAFKA_USER="${KAFKA_USER:-kafka}"
KAFKA_GROUP="${KAFKA_GROUP:-kafka}"
KAFKA_INSTALL_DIR="${KAFKA_INSTALL_DIR:-/opt/kafka}"
KAFKA_CONFIG_DIR="${KAFKA_CONFIG_DIR:-/etc/kafka/kraft}"
KAFKA_DATA_DIR="${KAFKA_DATA_DIR:-/var/lib/kafka/kraft-combined-logs}"
KAFKA_BROKER_PORT="${KAFKA_BROKER_PORT:-9092}"
KAFKA_LISTEN_ADDRESS="${KAFKA_LISTEN_ADDRESS:-0.0.0.0}"
KAFKA_ADVERTISED_HOST="${KAFKA_ADVERTISED_HOST:-127.0.0.1}"
KAFKA_ARCHIVE_URL="${KAFKA_ARCHIVE_URL:-https://archive.apache.org/dist/kafka/${KAFKA_VERSION}/kafka_${KAFKA_SCALA_VERSION}-${KAFKA_VERSION}.tgz}"

log() { printf '%s\n' "$*" >&2; }
die() { log "error: $*"; exit 1; }

require_root() {
  [ "$(id -u)" -eq 0 ] || die "run as root"
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

require_var() {
  local name="$1"
  local value="${!name:-}"
  [ -n "${value}" ] || die "${name} is required"
}

install_java_if_missing() {
  command -v java >/dev/null 2>&1 && return

  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    apt-get install -y --no-install-recommends openjdk-21-jre-headless curl ca-certificates tar
    return
  fi

  if command -v apk >/dev/null 2>&1; then
    apk add --no-cache openjdk21-jre curl ca-certificates tar bash coreutils
    return
  fi

  die "java not found and no supported package manager detected"
}

ensure_user_group() {
  getent group "${KAFKA_GROUP}" >/dev/null 2>&1 \
    || groupadd --system "${KAFKA_GROUP}"

  id "${KAFKA_USER}" >/dev/null 2>&1 \
    || useradd --system --gid "${KAFKA_GROUP}" --home-dir /nonexistent --shell /usr/sbin/nologin "${KAFKA_USER}"
}

install_kafka() {
  local tmp_archive
  tmp_archive="$(mktemp /tmp/kafka-broker.XXXXXX.tgz)"

  log "downloading ${KAFKA_ARCHIVE_URL}"
  curl -fsSL "${KAFKA_ARCHIVE_URL}" -o "${tmp_archive}"

  rm -rf "${KAFKA_INSTALL_DIR}"
  mkdir -p "${KAFKA_INSTALL_DIR}"
  tar -xzf "${tmp_archive}" --strip-components=1 -C "${KAFKA_INSTALL_DIR}"
  rm -f "${tmp_archive}"

  chown -R "${KAFKA_USER}:${KAFKA_GROUP}" "${KAFKA_INSTALL_DIR}"
}

write_broker_config() {
  mkdir -p "${KAFKA_CONFIG_DIR}" "${KAFKA_DATA_DIR}"
  chown -R "${KAFKA_USER}:${KAFKA_GROUP}" "${KAFKA_CONFIG_DIR}" "${KAFKA_DATA_DIR}"

  cat > "${KAFKA_CONFIG_DIR}/server.properties" <<EOF_CFG
# Broker-only node — no controller role.
process.roles=broker
node.id=${KAFKA_NODE_ID}
controller.quorum.voters=${KAFKA_QUORUM_VOTERS}

listeners=PLAINTEXT://${KAFKA_LISTEN_ADDRESS}:${KAFKA_BROKER_PORT}
advertised.listeners=PLAINTEXT://${KAFKA_ADVERTISED_HOST}:${KAFKA_BROKER_PORT}
inter.broker.listener.name=PLAINTEXT
controller.listener.names=CONTROLLER
listener.security.protocol.map=PLAINTEXT:PLAINTEXT,CONTROLLER:PLAINTEXT

log.dirs=${KAFKA_DATA_DIR}
num.partitions=3
default.replication.factor=1
offsets.topic.replication.factor=1
transaction.state.log.replication.factor=1
transaction.state.log.min.isr=1
min.insync.replicas=1
group.initial.rebalance.delay.ms=0
auto.create.topics.enable=false
delete.topic.enable=true
EOF_CFG

  chown "${KAFKA_USER}:${KAFKA_GROUP}" "${KAFKA_CONFIG_DIR}/server.properties"
  chmod 0640 "${KAFKA_CONFIG_DIR}/server.properties"
}

format_storage() {
  log "formatting KRaft storage with existing cluster id ${KAFKA_CLUSTER_ID}"
  "${KAFKA_INSTALL_DIR}/bin/kafka-storage.sh" format \
    -t "${KAFKA_CLUSTER_ID}" \
    -c "${KAFKA_CONFIG_DIR}/server.properties" \
    --ignore-formatted
}

write_systemd_unit() {
  cat > /etc/systemd/system/kafka-kraft.service <<EOF_UNIT
[Unit]
Description=Apache Kafka (KRaft broker)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${KAFKA_USER}
Group=${KAFKA_GROUP}
ExecStart=${KAFKA_INSTALL_DIR}/bin/kafka-server-start.sh ${KAFKA_CONFIG_DIR}/server.properties
ExecStop=${KAFKA_INSTALL_DIR}/bin/kafka-server-stop.sh
Restart=always
RestartSec=5
LimitNOFILE=200000
TimeoutStopSec=180

[Install]
WantedBy=multi-user.target
EOF_UNIT
}

start_service() {
  systemctl daemon-reload
  systemctl enable --now kafka-kraft
}

main() {
  require_root
  require_var KAFKA_NODE_ID
  require_var KAFKA_CLUSTER_ID
  require_var KAFKA_QUORUM_VOTERS
  require_cmd curl
  require_cmd tar
  install_java_if_missing
  ensure_user_group
  install_kafka
  write_broker_config
  format_storage
  write_systemd_unit
  start_service

  log "kafka broker node ${KAFKA_NODE_ID} installed and started"
  log "validate with: systemctl status kafka-kraft --no-pager"
}

main "$@"
