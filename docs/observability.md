# Observability

## Logging

### Log level

Set via `RUST_LOG`:

```bash
RUST_LOG=info           # production default
RUST_LOG=debug          # verbose, includes message details
RUST_LOG=trace          # very verbose, includes raw Kafka operations
RUST_LOG=webhook_relay=debug,info  # per-module: debug for serve, info for everything else
```

Valid levels: `error`, `warn`, `info`, `debug`, `trace`.

### What each role logs

**serve**:
- Startup: bound address, active profile, enabled sources, ingress adapter
- Per request: method, path, source, event type, dedup result, publish result
- Auth failures: source name and failure reason (no payload body logged)
- Rate limit hits: source and IP (redacted if proxied)
- Kafka publish: topic, success/failure, retry count

**relay**:
- Startup: topics consumed, output topic, group ID
- Per message: source topic, offset, output topic, forwarding result
- Kafka consumer events: rebalance, partition assignment

**smash**:
- Startup: active profile, egress adapters, routes, consumer group
- Per envelope: event ID, source, event type, route matched, adapter deliveries
- Delivery results: adapter ID, status, retry count
- DLQ events: event ID, adapter ID, error reason

### Log format

Logs are structured key-value lines via `tracing`. In production, pipe to a JSON formatter or use a log aggregator (journald, Loki, CloudWatch):

```bash
# Follow serve logs via journald
journalctl -u hook-serve -f

# Follow all hook roles
journalctl -u hook-serve -u hook-relay -u hook-smash -f
```

---

## Health Endpoints

serve exposes two HTTP endpoints when `http_webhook_ingress` is active:

### `GET /health`

Liveness probe. Always returns `200 OK` when the process is running.

```bash
curl http://localhost:8080/health
# → 200 OK
```

### `GET /ready`

Readiness probe. Returns `200 OK` when the Kafka producer is connected and ready. Returns `503` when the producer is not ready (e.g. broker unreachable).

```bash
curl http://localhost:8080/ready
# → 200 OK  {"status":"ready","profile":"default-openclaw","validation_mode":"strict"}
# → 503     {"status":"not_ready","reason":"kafka producer not connected"}
```

Use `/ready` for load balancer health checks and container orchestrator readiness gates.

---

## Kafka Consumer Group Lag

Consumer group lag is the primary operational metric for relay and smash. Zero lag means all published events have been consumed. Growing lag means a role is falling behind or has stopped.

Check lag for all groups:

```bash
/opt/kafka/bin/kafka-consumer-groups.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --describe --all-groups
```

Or for a specific group:

```bash
# relay consumer group
/opt/kafka/bin/kafka-consumer-groups.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --describe --group hook-relay

# smash consumer group
/opt/kafka/bin/kafka-consumer-groups.sh \
  --bootstrap-server 127.0.0.1:9092 \
  --describe --group hook-smash
```

The `LAG` column shows the number of unconsumed messages per partition.

---

## Watchdog Heartbeat

The Firecracker watchdog writes a JSON state snapshot every cycle to `{log_dir}/last_state.json` (default: `/var/log/firecracker/watchdog/last_state.json`):

```json
{
  "timestamp": "2026-03-04T12:00:00Z",
  "relay": {
    "ping": true,
    "port_open": true,
    "service_active": true,
    "process_state": "S"
  },
  "brokers": [
    { "id": "kafka", "port_open": true, "service_active": true }
  ],
  "memory_available_mb": 4096,
  "load_1m": 0.12
}
```

Quick status:

```bash
firecracker/watchdog/status.sh
```

---

## Metrics (Spec)

The following metrics are defined in the spec and planned for implementation. They are the intended Prometheus/OpenMetrics surface:

**Serve metrics:**

| Metric | Labels | Description |
|---|---|---|
| `serve_ingress_events_total` | `adapter`, `source` | Total inbound events accepted |
| `serve_publish_success_total` | `topic` | Successful Kafka publishes |
| `serve_publish_failure_total` | `topic`, `reason` | Failed Kafka publishes |

**Smash metrics:**

| Metric | Labels | Description |
|---|---|---|
| `smash_consume_events_total` | `topic` | Total envelopes consumed from core |
| `smash_egress_success_total` | `adapter` | Successful deliveries |
| `smash_egress_failure_total` | `adapter`, `reason` | Failed deliveries (including retried) |
| `smash_commit_total` | `topic` | Successful Kafka offset commits |
| `smash_dlq_total` | `topic` | Envelopes sent to DLQ |

Until the metrics endpoint is implemented, use log parsing and consumer group lag as proxies for these values.

---

## Tracing

The `trace_id` field in `EventEnvelope.meta` correlates an event across the full pipeline. When set, it appears in logs at all three stages (serve, relay, smash), making it possible to trace a single event from receipt to delivery.

To search logs by trace ID:

```bash
journalctl -u hook-serve -u hook-relay -u hook-smash | grep "trace_id=req-abc123"
```

---

## DLQ Monitoring

A non-empty DLQ requires attention. Monitor it with:

```bash
# Count messages in DLQ
kcat -b 127.0.0.1:9092 -t webhooks.dlq -o beginning -e -q | wc -l

# Inspect DLQ messages
kcat -b 127.0.0.1:9092 -t webhooks.dlq -o beginning -e -J \
  | jq '{failed_at: .payload | fromjson | .failed_at, error: .payload | fromjson | .error, source: .payload | fromjson | .envelope.source, event_type: .payload | fromjson | .envelope.event_type}'
```

See the `pipeline-debug` skill for DLQ replay instructions.

---

## External Blackbox Checks

For uptime monitoring from outside the deployment:

```bash
# Run on a separate host
# Probes /webhook/github (expect 401) and / (expect 200)
BLACKBOX_BASE_URL=https://relay.example.com \
  firecracker/watchdog/external-blackbox.sh
```

The external checker can be installed as a systemd timer on a monitoring host using `firecracker/systemd/external-blackbox.service` and `.timer`.

---

## Alerting

The watchdog alert system (`firecracker/watchdog/alert.sh`) provides:

- **Webhook delivery** — POST JSON alert to `ALERT_WEBHOOK_URL`
- **Email delivery** — via `sendmail` or `mail` to `ALERT_EMAIL_TO`
- **Per-event cooldown** — 300-second cooldown prevents alert floods for the same event key

Configure in `/etc/firecracker/alerts.env`:

```bash
ALERT_WEBHOOK_URL=https://hooks.slack.com/services/...
ALERT_WEBHOOK_BEARER_TOKEN=...
ALERT_EMAIL_TO=ops@example.com
```
