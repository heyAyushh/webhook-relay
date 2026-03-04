---
name: pipeline-debug
description: >
  Trace a webhook through the full hook pipeline (serve → relay → smash),
  inspect Kafka topic contents, check consumer group lag, and replay messages
  from the DLQ. Use when a webhook is lost, delayed, or failing delivery, or
  when diagnosing any serve/relay/smash runtime issue.
---

# Pipeline Debug

## Pipeline Overview

```
External → serve (webhook-relay) → webhooks.<source> → relay → webhooks.core → smash → Destination
                                                                             ↘ webhooks.dlq (on failure)
```

Each stage has its own logs and Kafka consumer group. Work through the stages in order.

---

## 1) Verify Serve Received the Webhook

Check serve logs for the incoming request:

```bash
# If running via systemd
journalctl -u hook-serve -n 200 --no-pager

# If running directly
# Logs go to stdout — check your process manager or redirect
```

Look for:
- `POST /webhook/<source>` — request received
- `signature ok` or `hmac ok` — auth passed
- `published` or `enqueued` — message sent to Kafka

**Auth failure (401 response)**: HMAC secret mismatch. Check:
```bash
echo $HMAC_SECRET_GITHUB   # must match what GitHub has configured
```

**No log at all**: the request never reached serve. Check network, firewall, and that serve is listening on the right port/address.

---

## 2) Inspect the Source Topic

Consume the last N messages from the source topic:

```bash
# Using kcat (recommended)
kcat -b 127.0.0.1:9092 -t webhooks.github -o -5 -e -J | jq .

# Using kafka-topics CLI
/opt/kafka/bin/kafka-console-consumer.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --topic webhooks.github \
  --from-beginning \
  --max-messages 5
```

If messages appear here, serve is working. If not, the issue is before Kafka.

Check topic offsets (producer high-water mark):

```bash
/opt/kafka/bin/kafka-run-class.sh kafka.tools.GetOffsetShell \
  --bootstrap-server 127.0.0.1:9092 \
  --topic webhooks.github
```

---

## 3) Verify Relay Is Running and Consuming

Check relay logs:

```bash
journalctl -u hook-relay -n 200 --no-pager
```

Look for:
- `consuming topics` — relay started
- `consumed message` / `forwarding to core` — relay is processing
- Any `error` or `lag` entries

Check relay consumer group lag:

```bash
/opt/kafka/bin/kafka-consumer-groups.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --describe --group hook-relay
```

`LAG` column should be 0 or low. A growing lag means relay is falling behind or stalled.

If relay consumer group doesn't exist yet, relay has never started or hasn't consumed any messages.

---

## 4) Inspect the Core Topic

```bash
kcat -b 127.0.0.1:9092 -t webhooks.core -o -5 -e -J | jq .
```

Messages should appear here after relay processes them. The envelope includes:
- `source` — which webhook source (e.g. `github`)
- `event_type` — e.g. `pull_request.opened`
- `payload` — sanitized JSON body
- `meta` — dedup key, timestamp, flags

If messages are in the source topic but not in core, relay is the problem — check relay logs and lag.

---

## 5) Verify Smash Is Running and Consuming

Check smash logs:

```bash
journalctl -u hook-smash -n 200 --no-pager
```

Look for:
- `consuming` / `delivering` — smash is processing
- `delivery ok` / `200` — successful delivery to destination
- `retry` / `failed` / `dlq` — delivery failure

Check smash consumer group lag:

```bash
/opt/kafka/bin/kafka-consumer-groups.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --describe --group hook-smash
```

---

## 6) Check the DLQ

Messages land in `webhooks.dlq` when smash exhausts retries.

```bash
# Inspect DLQ contents
kcat -b 127.0.0.1:9092 -t webhooks.dlq -o beginning -e -J | jq .

# Count DLQ messages
kcat -b 127.0.0.1:9092 -t webhooks.dlq -o beginning -e -q | wc -l
```

DLQ messages include the original envelope plus a `dlq_reason` field explaining why delivery failed.

---

## 7) Replay from DLQ

To replay DLQ messages back through smash, produce them to `webhooks.core`:

```bash
# Re-publish all DLQ messages to core (kcat)
kcat -b 127.0.0.1:9092 -t webhooks.dlq -o beginning -e \
  | kcat -b 127.0.0.1:9092 -t webhooks.core -P
```

Before replaying, fix whatever caused the DLQ failure (bad token, unreachable destination, etc.).

To replay a single message, use `jq` to extract and re-produce it:

```bash
kcat -b 127.0.0.1:9092 -t webhooks.dlq -o beginning -e -J \
  | jq -r 'select(.payload | fromjson | .meta.dedup_key == "github:abc123:opened:42") | .payload' \
  | kcat -b 127.0.0.1:9092 -t webhooks.core -P
```

---

## 8) List All Consumer Groups

```bash
/opt/kafka/bin/kafka-consumer-groups.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --list
```

```bash
# Check lag across all groups at once
/opt/kafka/bin/kafka-consumer-groups.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --describe --all-groups
```

---

## Common Failures and Fixes

| Symptom | Likely cause | Fix |
|---|---|---|
| 401 from serve | HMAC secret mismatch | Correct `HMAC_SECRET_<SOURCE>` env var |
| Message in source topic, not in core | Relay not running or wrong `--topics` | Start relay with correct topic list |
| Message in core, not delivered | Smash not running or destination unreachable | Check smash logs and destination health |
| Growing lag on relay or smash | Kafka throughput issue or consumer crash | Check process status, restart if needed |
| Message in DLQ | Delivery failure after retries | Fix destination, then replay |
| `UnknownTopicOrPartition` | Topic not created | Run `kafka-topic-setup` skill |
| Smash starts then exits | Contract validation failure | Check smash startup logs for validation error codes |
