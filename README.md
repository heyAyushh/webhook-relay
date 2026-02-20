# Webhook Relay -> AutoMQ -> OpenClaw

Production-oriented Rust event pipeline:

1. `webhook-relay` (public VM): validates webhook auth and publishes normalized envelopes to AutoMQ/Kafka.
2. `automq-consumer` (local, outbound only): consumes `webhooks.*`, forwards to OpenClaw `/hooks/agent`.
3. Orchestrator session (`coder:orchestrator`): receives all events and coordinates worker subagents.

## Architecture

```text
internet -> nginx:443 -> webhook-relay -> AutoMQ (mTLS) -> automq-consumer -> OpenClaw /hooks/agent
```

## Repository Layout

- `src/`: `webhook-relay` app (Axum ingress + Kafka producer)
- `apps/automq-consumer/`: Kafka consumer + retrying OpenClaw forwarder + DLQ producer
- `crates/relay-core/`: shared source models, signature checks, sanitizer, and parity helpers
- `deploy/nginx/webhook-relay.conf`: TLS termination/proxy config
- `docker-compose.yml`: nginx + relay + consumer stack
- `scripts/gen-certs.sh`: mTLS cert generation (CA, relay cert, consumer cert)
- `systemd/`: service units
- `memory/coder-tasks.md`: orchestrator task board

## Relay API

### `POST /webhook/{source}`

Supported `source` values:

- `github`
- `linear`

Behavior:

- verifies source auth (`X-Hub-Signature-256`, `Linear-Signature`)
- parses JSON payload
- derives `event_type`
- publishes envelope to topic `webhooks.{source}` asynchronously
- returns `200` fast when accepted

Event compatibility:

- GitHub: any `X-GitHub-Event` is accepted; `action` is appended when present.
- Linear: any `type` is accepted; `action` is appended when present.

Other endpoints:

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

- IP rate limit: `100 req/min` (`tower-governor`)
- Source rate limit: `500 req/min`
- Body limit: `1 MB`
- Fail-fast auth reject (`401`) with no payload logging
- AutoMQ communication over mTLS
- Consumer has no inbound ports

## Configuration

Use `.env.default` as your base:

```bash
cp .env.default .env
```

Required values to set at minimum:

- `KAFKA_BROKERS`
- `HMAC_SECRET_GITHUB`
- `HMAC_SECRET_LINEAR`
- `OPENCLAW_WEBHOOK_TOKEN`

## Build and Test

Prerequisites:

- Rust stable
- OpenSSL
- CMake (for `rdkafka-sys`)

Run:

```bash
cargo test --workspace
cargo build --workspace --release
```

## Zero-Lift Init

Bootstrap everything with one command:

```bash
scripts/init.sh --up
```

What this does:

- creates `.env` from `.env.default` if missing
- generates strong random secrets for placeholder values
- generates AutoMQ mTLS certs via `scripts/gen-certs.sh`
- generates local TLS certs for nginx (`certs/tls.crt`, `certs/tls.key`)
- writes ready-to-use systemd env files to `deploy/env/`
- optionally starts stack with `docker compose up --build -d`

## mTLS Certificates

Generate local cert material:

```bash
scripts/gen-certs.sh
```

Outputs:

- `certs/ca.crt`, `certs/ca.key`
- `certs/relay.crt`, `certs/relay.key`
- `certs/consumer.crt`, `certs/consumer.key`

## Run with Docker Compose

```bash
docker compose up --build
```

Services:

- `nginx` on `443`
- `webhook-relay`
- `automq-consumer`

## Systemd Deployment

Install units:

- `systemd/webhook-relay.service`
- `systemd/automq-consumer.service`

Then:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now webhook-relay
sudo systemctl enable --now automq-consumer
```

## Orchestrator Contract

Consumer forwards all events to the same session:

- `agentId = coder`
- `sessionKey = coder:orchestrator`
- `channel = telegram`
- `to = -1003734912836:topic:2`

Task board file:

- `memory/coder-tasks.md`
