---
name: kafka-topic-setup
description: >
  Create and configure the standard Kafka topic set required by hook
  (serve source topics, webhooks.core, webhooks.dlq). Use when setting up a
  new environment, after provisioning a fresh Kafka cluster, or when topics
  need to be recreated with corrected settings.
---

# Kafka Topic Setup

## Standard Topic Set

Hook requires these topics before serve, relay, or smash can run:

| Topic | Consumer | Purpose |
|---|---|---|
| `webhooks.<source>` (e.g. `webhooks.github`) | relay | Per-source ingress topics written by serve |
| `webhooks.core` | smash | Normalised core topic written by relay |
| `webhooks.dlq` | ops | Dead letter queue for failed smash deliveries |

`auto.create.topics.enable=false` is the default in the bootstrap script — topics must be created explicitly.

---

## Create All Topics

Run on the Kafka guest (or any host that can reach the broker):

```bash
scripts/kafka-topic-setup.sh
```

Or use the script with custom settings:

```bash
KAFKA_BOOTSTRAP=127.0.0.1:9092 \
SOURCES="github linear" \
  skills/kafka-topic-setup/scripts/create-hook-topics.sh
```

---

## Manual Topic Creation

Replace `127.0.0.1:9092` with your bootstrap server.

### Source topics (one per active webhook source)

```bash
/opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --create --if-not-exists \
  --topic webhooks.github \
  --partitions 3 \
  --replication-factor 1 \
  --config retention.ms=604800000 \
  --config cleanup.policy=delete

/opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --create --if-not-exists \
  --topic webhooks.linear \
  --partitions 3 \
  --replication-factor 1 \
  --config retention.ms=604800000 \
  --config cleanup.policy=delete
```

### Core topic

```bash
/opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --create --if-not-exists \
  --topic webhooks.core \
  --partitions 3 \
  --replication-factor 1 \
  --config retention.ms=604800000 \
  --config cleanup.policy=delete
```

### DLQ topic

```bash
/opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --create --if-not-exists \
  --topic webhooks.dlq \
  --partitions 1 \
  --replication-factor 1 \
  --config retention.ms=2592000000 \
  --config cleanup.policy=delete
```

DLQ uses longer retention (30 days) and fewer partitions — messages land there infrequently and are consumed manually.

---

## Validate Topics Exist

```bash
/opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --list | grep webhooks
```

Expected output:

```
webhooks.core
webhooks.dlq
webhooks.github
webhooks.linear
```

Describe a topic to verify partition/replication config:

```bash
/opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --describe --topic webhooks.core
```

---

## Multi-Broker Replication

With more than one broker (see `kafka-add-broker` skill), increase replication:

```bash
# Increase replication factor for core and DLQ
for topic in webhooks.core webhooks.dlq; do
  /opt/kafka/bin/kafka-configs.sh \
    --bootstrap-server 127.0.0.1:9092 \
    --alter --topic "${topic}" \
    --replication-factor 2
done
```

Also update `min.insync.replicas` for durability guarantees:

```bash
/opt/kafka/bin/kafka-configs.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --alter --entity-type topics --entity-name webhooks.core \
  --add-config min.insync.replicas=2
```

---

## Adding a New Source Topic

When adding a new webhook source, create its topic before starting serve:

```bash
/opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --create --if-not-exists \
  --topic webhooks.<newsource> \
  --partitions 3 \
  --replication-factor 1 \
  --config retention.ms=604800000
```

Then add the topic to the relay invocation:

```bash
hook relay \
  --topics webhooks.github,webhooks.linear,webhooks.<newsource> \
  --output-topic webhooks.core
```

---

## Troubleshooting

- **`UnknownTopicOrPartitionException`**: topic not created yet — run this skill first.
- **`LEADER_NOT_AVAILABLE`** on create: transient on single-node clusters, retry after a few seconds.
- **`replication.factor` exceeds broker count**: lower the factor or add brokers first.
- **serve starts but messages not appearing in core**: check that relay is running and includes the source topic in `--topics`.
