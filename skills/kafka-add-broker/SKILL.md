---
name: kafka-add-broker
description: "Add Kafka broker nodes to an existing KRaft cluster (typically bootstrapped via kafka-kraft-firecracker) or configure hook to connect to an external Kafka broker or cluster. Use when scaling out brokers, adding redundancy, connecting to external Kafka (AWS MSK, Confluent, self-hosted), or pointing hook serve/relay/smash at a new message broker."
---

# Kafka Add Broker

## Scope

Two modes:

- **Mode A — Join existing cluster**: Provision a new broker-only KRaft node and join it to the cluster created by `kafka-kraft-firecracker`. The existing node (node 1) remains the controller; new nodes are broker-only.
- **Mode B — External Kafka**: Skip local broker provisioning and point hook to an external Kafka cluster.

---

## Mode A: Add Broker to Existing KRaft Cluster

### Prerequisites

- Existing KRaft cluster running (from `kafka-kraft-firecracker` skill or equivalent).
- Existing cluster ID — retrieve from the controller node:

```bash
# On the existing controller guest (node 1)
cat /var/lib/kafka/kraft-combined-logs/meta.properties | grep cluster.id
```

- A new Firecracker VM booted and reachable (see `kafka-kraft-firecracker` skill sections 2–3 for TAP + boot steps). Use a different TAP and IP from the controller node.
- Unique `node.id` for the new broker (e.g. `2`, `3`).

### 1) Host Network for New Broker VM

Provision a second TAP for the new broker VM. Adjust the example for your numbering:

```bash
TAP_NAME=tap-kafka1 \
GUEST_IP=172.16.41.2 \
HOST_IP=172.16.41.1/24 \
  skills/kafka-kraft-firecracker/scripts/setup-firecracker-kafka-tap.sh
```

Or manually:

```bash
ip tuntap add tap-kafka1 mode tap
ip addr add 172.16.41.1/24 dev tap-kafka1
ip link set tap-kafka1 up
iptables -t nat -A POSTROUTING -s 172.16.41.0/24 -j MASQUERADE
iptables -A FORWARD -i tap-kafka1 -j ACCEPT
iptables -A FORWARD -o tap-kafka1 -m state --state RELATED,ESTABLISHED -j ACCEPT
```

### 2) Boot New Broker VM

Boot a Firecracker VM with the new TAP. The guest IP must be reachable from the controller guest IP (either same subnet or routed).

### 3) Bootstrap Broker-Only Node

Copy and run the broker bootstrap script inside the new guest as root:

```bash
cp skills/kafka-add-broker/scripts/bootstrap-kafka-broker.sh /tmp/
chmod +x /tmp/bootstrap-kafka-broker.sh

# Required env vars:
KAFKA_NODE_ID=2 \
KAFKA_CLUSTER_ID=<cluster-id-from-meta.properties> \
KAFKA_ADVERTISED_HOST=172.16.41.2 \
KAFKA_QUORUM_VOTERS="1@172.16.40.2:9093" \
  bash /tmp/bootstrap-kafka-broker.sh
```

Key env vars:

| Variable | Default | Description |
|---|---|---|
| `KAFKA_NODE_ID` | — | **Required.** Unique node ID; must not conflict with existing nodes. |
| `KAFKA_CLUSTER_ID` | — | **Required.** Must match the existing cluster's cluster ID. |
| `KAFKA_QUORUM_VOTERS` | — | **Required.** Controller quorum voter list, e.g. `1@172.16.40.2:9093`. |
| `KAFKA_ADVERTISED_HOST` | `127.0.0.1` | Guest TAP IP so other nodes and clients can reach this broker. |
| `KAFKA_BROKER_PORT` | `9092` | Broker listener port. |
| `KAFKA_LISTEN_ADDRESS` | `0.0.0.0` | Bind address. |
| `KAFKA_VERSION` | `4.0.0` | Must match the controller node's version. |
| `KAFKA_SCALA_VERSION` | `2.13` | Must match the controller node's version. |

### 4) Validate New Broker Joined

On the controller guest (node 1):

```bash
# List brokers registered in the cluster metadata
/opt/kafka/bin/kafka-broker-api-versions.sh --bootstrap-server 127.0.0.1:9092

# Check metadata for broker count
/opt/kafka/bin/kafka-topics.sh --bootstrap-server 127.0.0.1:9092 --describe --topic webhooks.core
```

Expected: new `node.id` appears in the broker list.

### 5) Register New Broker in Host Inventory

Add the new broker to `/etc/firecracker/brokers.json` so proxy-mux and the watchdog discover it:

```json
{
  "brokers": [
    {
      "id": "kafka",
      "node_id": 1,
      "ip": "172.16.40.2",
      "tap": "tap-kafka0",
      "socket": "/tmp/kafka-fc.sock",
      "host_proxy_port": 9092,
      "config": "/opt/firecracker/kafka/config.json",
      "rootfs": "/opt/firecracker/kafka/rootfs.ext4"
    },
    {
      "id": "kafka2",
      "node_id": 2,
      "ip": "172.16.41.2",
      "tap": "tap-kafka1",
      "socket": "/tmp/kafka2-fc.sock",
      "host_proxy_port": 9093,
      "config": "/opt/firecracker/kafka2/config.json",
      "rootfs": "/opt/firecracker/kafka2/rootfs.ext4"
    }
  ]
}
```

Then enable broker proxies so hook can reach both via localhost:

```bash
# /etc/firecracker/proxy-mux.env
FIRECRACKER_ENABLE_BROKER_PROXIES=true
```

Restart proxy-mux:

```bash
systemctl restart firecracker-proxy-mux.service
```

### 6) Update Topic Replication (Optional)

With multiple brokers, increase replication factors:

```bash
# On the controller guest
/opt/kafka/bin/kafka-configs.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --alter \
  --topic webhooks.core \
  --partitions 3

# Reassign replicas across brokers (generate reassignment plan first)
/opt/kafka/bin/kafka-reassign-partitions.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --topics-to-move-json-file /tmp/topics.json \
  --broker-list "1,2" \
  --generate
```

---

## Mode B: External Kafka

Use this when Kafka is managed externally (cloud MSK, Confluent, self-hosted cluster, etc.) and you only need to point hook at it.

### 1) Update `.env`

```bash
KAFKA_BROKERS=broker1.example.com:9092,broker2.example.com:9092

# If the external cluster requires TLS:
KAFKA_SECURITY_PROTOCOL=ssl
# KAFKA_ALLOW_PLAINTEXT must NOT be set to true for TLS

# If plaintext (explicit opt-in required):
# KAFKA_SECURITY_PROTOCOL=plaintext
# KAFKA_ALLOW_PLAINTEXT=true
```

### 2) Validate Connectivity

```bash
# From hook host — check TCP reachability
nc -zv broker1.example.com 9092

# From kcat (if available)
kcat -b broker1.example.com:9092 -L
```

### 3) Skip Broker Inventory / Proxy-Mux

External brokers do not go in `brokers.json`. The proxy-mux is for Firecracker-local VMs only. Leave `FIRECRACKER_ENABLE_BROKER_PROXIES=false` (default).

### 4) Run hook

No changes to hook CLI — `KAFKA_BROKERS` is all that hook reads:

```bash
hook serve --app default-openclaw
hook relay --topics webhooks.github,webhooks.linear --output-topic webhooks.core
hook smash --app default-openclaw
```

---

## Hardening (Both Modes)

- Never expose controller port (`9093`) outside private network.
- For cross-VM broker communication, restrict firewall to the exact TAP/guest CIDRs.
- Match `KAFKA_VERSION` and `KAFKA_SCALA_VERSION` across all nodes — mixed versions within a cluster will cause metadata errors.
- `auto.create.topics.enable=false` is the default; create topics explicitly.

## Troubleshooting

- **Broker does not join cluster**: verify `KAFKA_CLUSTER_ID` matches exactly and `KAFKA_QUORUM_VOTERS` IP/port is reachable from the new broker guest.
- **`node.id` conflict**: each node must have a unique ID; check `meta.properties` on all nodes.
- **Replication factor > broker count**: lower replication factor or add more brokers before creating topics.
- **Mixed version metadata errors**: re-run bootstrap with matching `KAFKA_VERSION`.
- **External broker unreachable**: check `KAFKA_SECURITY_PROTOCOL` matches the cluster config; verify TLS certs if applicable.
