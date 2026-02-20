# Webhook Relay -> AutoMQ -> OpenClaw

Production-oriented Rust event pipeline:

1. `webhook-relay` (public VM): validates webhook auth and publishes normalized envelopes to AutoMQ/Kafka.
2. `kafka-openclaw-hook` (local, outbound only): consumes `webhooks.*`, forwards to OpenClaw `/hooks/agent`.
3. Orchestrator session (`coder:orchestrator`): receives all events and coordinates worker subagents.

## Architecture

```text
internet -> nginx:443 -> webhook-relay -> AutoMQ (mTLS) -> kafka-openclaw-hook -> OpenClaw /hooks/agent
```

## Repository Layout

- `apps/README.md`: index of runtime application crates
- `crates/README.md`: index of shared library crates
- `src/`: `webhook-relay` app (Axum ingress + Kafka producer)
- `apps/kafka-openclaw-hook/`: Kafka consumer + retrying OpenClaw forwarder + DLQ producer
- `crates/relay-core/`: shared source models, signature checks, sanitizer, and parity helpers
- `deploy/nginx/webhook-relay.conf`: TLS termination/proxy config
- `docker-compose.yml`: relay deployment profile (nginx + relay)
- `docker-compose.dev.yml`: relay dev override profile (proxy trust defaults)
- `scripts/gen-certs.sh`: mTLS cert generation (CA, relay cert, consumer cert)
- `systemd/`: service units
- `memory/coder-tasks.md`: orchestrator task board

## Component Docs and Skills

- Workspace skill: `SKILL.md`
- Consumer app docs: `apps/kafka-openclaw-hook/README.md`
- Consumer app skill: `apps/kafka-openclaw-hook/SKILL.md`
- Shared crate docs: `crates/relay-core/README.md`
- Shared crate skill: `crates/relay-core/SKILL.md`

## Relay API

### `POST /webhook/{source}`

Supported `source` values:

- `github`
- `linear`

Behavior:

- verifies source auth (`X-Hub-Signature-256`, `Linear-Signature`)
- enforces Linear replay window via `webhookTimestamp`
- parses JSON payload
- derives `event_type`
- applies delivery dedup + per-entity cooldown
- sanitizes untrusted payload fields before publish
- publishes envelope to topic `webhooks.{source}` asynchronously
- returns `200` fast when accepted

Event compatibility:

- GitHub: any `X-GitHub-Event` is accepted; `action` is appended when present.
- Linear: any `type` is accepted; if `type` is missing, `Linear-Event` header is used; `action` is appended when present.

Compatibility references:

- GitHub events: <https://docs.github.com/en/webhooks/webhook-events-and-payloads>
- Linear webhooks: <https://linear.app/developers/webhooks>
- Linear schema objects: <https://studio.apollographql.com/public/Linear-Webhooks/variant/current/schema/reference/objects>

Compatibility tests:

- `src/sources/github.rs` -> `accepts_all_documented_github_app_events`
- `src/sources/linear.rs` -> `accepts_all_documented_linear_webhook_types`

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
- Delivery dedup TTL: `7d` (default)
- Per-entity cooldown: `30s` (default)
- Linear timestamp replay window: `60s` (default)
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

Useful optional relay controls:

- `RELAY_DEDUP_TTL_SECONDS` (default `604800`)
- `RELAY_COOLDOWN_SECONDS` (default `30`)
- `RELAY_ENFORCE_LINEAR_TIMESTAMP_WINDOW` (default `true`)
- `RELAY_LINEAR_TIMESTAMP_WINDOW_SECONDS` (default `60`)
- `RELAY_TRUST_PROXY_HEADERS` (default `false`)
- `RELAY_TRUSTED_PROXY_CIDRS` (default `127.0.0.1/32,::1/128`)

Useful optional consumer controls:

- `OPENCLAW_AGENT_ID` (default `coder`)
- `OPENCLAW_SESSION_KEY` (default `coder:orchestrator`)
- `OPENCLAW_WAKE_MODE` (default `now`)
- `OPENCLAW_NAME` (default `WebhookRelay`)
- `OPENCLAW_DELIVER` (default `true`)
- `OPENCLAW_CHANNEL` (default `telegram`)
- `OPENCLAW_TO` (default `-1003734912836:topic:2`)
- `OPENCLAW_MODEL` (default `anthropic/claude-sonnet-4-6`)
- `OPENCLAW_THINKING` (default `low`)
- `OPENCLAW_TIMEOUT_SECONDS` (default `600`)
- `OPENCLAW_MESSAGE_MAX_BYTES` (default `4000`)
- `OPENCLAW_HTTP_TIMEOUT_SECONDS` (default `20`)

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

HTTP behavior smoke test (against a running relay):

```bash
HMAC_SECRET_GITHUB=... HMAC_SECRET_LINEAR=... scripts/smoke-test-rust.sh --relay-url http://127.0.0.1:8080
```

## Zero-Lift Init

Bootstrap everything with one command:

```bash
scripts/init.sh --up
```

By default, this starts the `dev` compose profile (relay with dev overrides).
For explicit relay profile:

```bash
scripts/init.sh --up --profile relay
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

Relay profile (VM/public ingress, relay-only):
```bash
docker compose -f docker-compose.yml up --build
```

Relay dev override profile:

```bash
docker compose -f docker-compose.yml -f docker-compose.dev.yml up --build
```

Services:

- `nginx` on `443`
- `webhook-relay`

`kafka-openclaw-hook` runs as a native binary (for example via systemd), not in Docker.

## Systemd Deployment

Install units:

- `systemd/webhook-relay.service`
- `systemd/kafka-openclaw-hook.service`

Then:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now webhook-relay
sudo systemctl enable --now kafka-openclaw-hook
```

## Orchestrator Contract

Consumer forwards all events to the same session:

- `agentId = coder`
- `sessionKey = coder:orchestrator`
- `channel = telegram`
- `to = -1003734912836:topic:2`

Task board file:

- `memory/coder-tasks.md`
