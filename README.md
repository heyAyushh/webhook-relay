# webhook-relay

Rust webhook relay for GitHub and Linear events targeting OpenClaw, with signature verification, queue-backed delivery, deduplication, cooldown controls, sanitization, retries, metrics, and DLQ replay.

## Current Status

The Rust rewrite is active and runs as a standalone service.
Legacy shell/Python relay files are kept in this repository for parity references and migration support.

## What It Does

- Accepts GitHub and Linear webhook events on HTTP endpoints
- Verifies webhook signatures before parsing payloads
- Applies event/type/action filtering
- Enforces dedup and per-entity cooldown semantics
- Queues accepted events durably in `redb`
- Sanitizes untrusted payload content before forwarding to OpenClaw
- Retries transient forwarding failures with exponential backoff
- Moves exhausted failures to DLQ and supports replay
- Exposes Prometheus metrics and health/readiness endpoints

## Endpoints

- `POST /hooks/github-pr`
- `POST /hooks/linear`
- `GET /health`
- `GET /ready`
- `GET /metrics`
- `GET /admin/queue` (requires admin token)
- `GET /admin/dlq` (requires admin token)
- `POST /admin/dlq/replay/{event_id}` (requires admin token)

## Required Environment Variables

```bash
export OPENCLAW_GATEWAY_URL="https://<your-openclaw-gateway>"
export OPENCLAW_HOOKS_TOKEN="..."
export GITHUB_WEBHOOK_SECRET="..."
export LINEAR_WEBHOOK_SECRET="..."
```

## Optional Environment Variables

Parity-compatible controls:

- `WEBHOOK_DEDUP_RETENTION_DAYS` (default: `7`)
- `GITHUB_COOLDOWN_SECONDS` (default: `30`)
- `LINEAR_COOLDOWN_SECONDS` (default: `30`)
- `LINEAR_TIMESTAMP_WINDOW_SECONDS` (default: `60`)
- `LINEAR_ENFORCE_TIMESTAMP_CHECK` (default: `true`)
- `WEBHOOK_CURL_CONNECT_TIMEOUT_SECONDS` (default: `5`)
- `WEBHOOK_CURL_MAX_TIME_SECONDS` (default: `20`)
- `WEBHOOK_FORWARD_MAX_ATTEMPTS` (default: `5`)
- `WEBHOOK_FORWARD_INITIAL_BACKOFF_SECONDS` (default: `1`)
- `WEBHOOK_FORWARD_MAX_BACKOFF_SECONDS` (default: `30`)
- `LINEAR_AGENT_USER_ID` (default: unset)

Rust runtime controls:

- `WEBHOOK_BIND_ADDR` (default: `0.0.0.0:9000`)
- `WEBHOOK_DB_PATH` (default: `/tmp/webhook-relay/relay.redb`)
- `WEBHOOK_MAX_BODY_BYTES` (default: `524288`)
- `WEBHOOK_QUEUE_POLL_INTERVAL_MS` (default: `500`)
- `WEBHOOK_ADMIN_TOKEN` (default: unset; admin APIs disabled)
- `RUST_LOG` (default: `info`)

## Run Locally (Rust)

```bash
cargo run
```

## Test (TDD + parity modules)

```bash
cargo test
```

Rust parity smoke test:

```bash
GITHUB_WEBHOOK_SECRET=... \
LINEAR_WEBHOOK_SECRET=... \
OPENCLAW_HOOKS_TOKEN=... \
scripts/smoke-test-rust.sh
```

## Docker

Build image:

```bash
docker build -t webhook-relay:dev .
```

Run compose stack (relay + mock OpenClaw):

```bash
docker compose up --build
```

Relay listens on `http://127.0.0.1:9000`.

## Repository Layout

- `src/`: Rust relay implementation
- `Dockerfile`: production-focused container image
- `docker-compose.yml`: local integration stack
- `firecracker/`: Firecracker runtime templates and service unit
- `proposal.md`: rewrite proposal and parity contract
- `references/`: operational and integration runbooks
- `SKILL.md`: repository skill metadata
- `scripts/`: legacy relay + sanitizer + smoke test assets retained for migration

## Legacy Assets (Preserved)

The following remain available for migration/parity work:

- `hooks.yaml`
- `scripts/relay-github.sh`
- `scripts/relay-linear.sh`
- `scripts/sanitize-payload.py`
- `scripts/smoke-test.sh`

## Firecracker Helpers

- Build rootfs/data images: `scripts/build-firecracker-rootfs.sh`
- Run Firecracker with config: `scripts/run-firecracker.sh`
- Runtime templates: `firecracker/firecracker-config.template.json`

## Security Model

1. Verify authenticity (HMAC signatures)
2. Filter unsupported events early
3. Dedup and cooldown to reduce replay/storm impact
4. Sanitize user text before LLM-facing downstream calls
5. Queue and retry transient failures; isolate hard failures in DLQ
