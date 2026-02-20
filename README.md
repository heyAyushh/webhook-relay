# webhook-relay

Hardened webhook pipeline for OpenClaw:

1. `webhook-relay` (public VM) validates inbound webhooks and publishes normalized envelopes to Kafka/AutoMQ.
2. `automq-consumer` (outbound only) consumes envelopes and POSTs to local OpenClaw `/hooks/agent`.
3. `coder:orchestrator` receives all events in one session and coordinates worker subagents.

## Workspace Layout

- `src/`: `webhook-relay` binary (Axum + Kafka producer)
- `apps/automq-consumer/`: consumer binary (Kafka consumer + OpenClaw forwarder + DLQ)
- `crates/relay-core/`: shared source/signature/model logic
- `scripts/gen-certs.sh`: mTLS cert generation (CA, relay cert, consumer cert)
- `deploy/nginx/webhook-relay.conf`: TLS termination + proxy
- `systemd/`: service units for relay and consumer
- `memory/coder-tasks.md`: shared orchestrator task board

## Legacy Scripts

Legacy shell/Python scripts are still kept in `scripts/` for migration fallback and parity reference.
They are not required for the new Rust runtime path (`webhook-relay` + `automq-consumer`).

## Relay API

- `POST /webhook/{source}`
  - `source`: `github`, `linear`, `gmail`
  - validates source signature/token
  - normalizes payload into envelope and queues async Kafka publish
  - returns `200 OK` quickly on accepted enqueue
- `GET /health`
- `GET /ready`

Unknown source path returns `404`.

## Envelope Schema

```json
{
  "id": "uuid-v4",
  "source": "github",
  "event_type": "pull_request.opened",
  "received_at": "2026-02-20T14:00:00Z",
  "payload": {}
}
```

## Security Controls

- Per-IP rate limit: `100 req/min` (`tower-governor`)
- Per-source rate limit: `500 req/min`
- Body size limit: `1 MB`
- HMAC/token verification before JSON parse acceptance
- No payload body logging on auth failures
- mTLS between relay/consumer and AutoMQ

## Environment Variables

### `webhook-relay`

Required:

- `KAFKA_BROKERS`
- `KAFKA_TLS_CERT`
- `KAFKA_TLS_KEY`
- `KAFKA_TLS_CA`
- `HMAC_SECRET_GITHUB`
- `HMAC_SECRET_LINEAR`
- `HMAC_SECRET_GMAIL`

Optional:

- `RELAY_BIND` (default: `0.0.0.0:8080`)
- `RELAY_MAX_PAYLOAD_BYTES` (default: `1048576`)
- `RELAY_IP_RATE_PER_MINUTE` (default: `100`)
- `RELAY_SOURCE_RATE_PER_MINUTE` (default: `500`)
- `RELAY_PUBLISH_QUEUE_CAPACITY` (default: `4096`)
- `RELAY_PUBLISH_MAX_RETRIES` (default: `5`)
- `RELAY_PUBLISH_BACKOFF_BASE_MS` (default: `200`)
- `RELAY_PUBLISH_BACKOFF_MAX_MS` (default: `5000`)

### `automq-consumer`

Required:

- `KAFKA_BROKERS`
- `KAFKA_TLS_CERT`
- `KAFKA_TLS_KEY`
- `KAFKA_TLS_CA`
- `KAFKA_TOPICS`
- `OPENCLAW_WEBHOOK_URL`
- `OPENCLAW_WEBHOOK_TOKEN`

Optional:

- `KAFKA_GROUP_ID` (default: `openclaw-consumer`)
- `KAFKA_DLQ_TOPIC` (default: `webhooks.dlq`)
- `CONSUMER_MAX_RETRIES` (default: `5`)
- `CONSUMER_BACKOFF_BASE_SECONDS` (default: `1`)
- `CONSUMER_BACKOFF_MAX_SECONDS` (default: `30`)

## Local Build/Test

Prerequisites:

- Rust stable
- OpenSSL
- CMake (required by `rdkafka-sys`)

Run:

```bash
cargo test --workspace
cargo build --workspace --release
```

## Certificates (mTLS)

Generate CA and client certs:

```bash
scripts/gen-certs.sh
```

Outputs:

- `certs/ca.crt`, `certs/ca.key`
- `certs/relay.crt`, `certs/relay.key`
- `certs/consumer.crt`, `certs/consumer.key`

## Docker Compose (VM stack)

`docker-compose.yml` includes:

- `nginx` (TLS termination on `443`)
- `webhook-relay`
- `automq-consumer`

Run:

```bash
docker compose up --build
```

## Systemd Units

- `systemd/webhook-relay.service`
- `systemd/automq-consumer.service`

Install to `/etc/systemd/system/`, then:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now webhook-relay
sudo systemctl enable --now automq-consumer
```

## OpenClaw Orchestrator

Use fixed session key: `coder:orchestrator`.

Consumer forwards a single payload shape to `/hooks/agent` with:

- `agentId = coder`
- `sessionKey = coder:orchestrator`
- `channel = telegram`
- `to = -1003734912836:topic:2`

Shared board file for cross-session awareness:

- `memory/coder-tasks.md`
