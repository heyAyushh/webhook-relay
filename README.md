# webhook-relay

Relay GitHub and Linear webhooks to an OpenClaw gateway with signature checks, replay protection, cooldown throttling, and payload sanitization for LLM safety.

## What it does

- Accepts webhook events through `adnanh/webhook` (`hooks.yaml`)
- Routes GitHub PR/review/comment and Linear issue/comment events to relay scripts
- Verifies signatures (`payload-hmac-sha256` for GitHub via webhook, HMAC validation for Linear in script)
- Deduplicates repeated deliveries and applies per-entity cooldown windows
- Sanitizes user-controlled text to reduce prompt-injection risk before forwarding
- Forwards sanitized payloads to OpenClaw hooks endpoints

## Repository layout

- `hooks.yaml`: webhook endpoint and trigger definitions
- `scripts/relay-github.sh`: GitHub relay, dedup, cooldown, forward
- `scripts/relay-linear.sh`: Linear relay, signature verification, dedup, cooldown, forward
- `scripts/sanitize-payload.py`: allowlist extraction, text fencing, injection pattern flagging, truncation
- `scripts/smoke-test.sh`: end-to-end signed webhook smoke test
- `references/`: setup and integration runbooks

## Prerequisites

- `webhook` (https://github.com/adnanh/webhook)
- `bash`, `curl`, `openssl`, `python3`

Install webhook on macOS:

```bash
brew install webhook
```

Alternative:

```bash
go install github.com/adnanh/webhook@latest
```

## Required environment variables

```bash
export GITHUB_WEBHOOK_SECRET="..."
export LINEAR_WEBHOOK_SECRET="..."
export OPENCLAW_HOOKS_TOKEN="..."
export OPENCLAW_GATEWAY_URL="https://<your-openclaw-gateway>"
```

Optional runtime controls:

- `WEBHOOK_DEDUP_DIR` (default: `/tmp/webhook-dedup`)
- `WEBHOOK_DEDUP_RETENTION_DAYS` (default: `7`)
- `GITHUB_COOLDOWN_SECONDS` (default: `30`)
- `LINEAR_COOLDOWN_SECONDS` (default: `30`)
- `LINEAR_TIMESTAMP_WINDOW_SECONDS` (default: `60`)
- `LINEAR_ENFORCE_TIMESTAMP_CHECK` (default: `true`)
- `WEBHOOK_CURL_CONNECT_TIMEOUT_SECONDS` (default: `5`)
- `WEBHOOK_CURL_MAX_TIME_SECONDS` (default: `20`)

## Run locally

```bash
webhook -hooks hooks.yaml -verbose -port 9000
```

Endpoints:

- `POST /hooks/github-pr`
- `POST /hooks/linear`

## Quick sanitizer check

```bash
echo '{"action":"opened"}' | python3 scripts/sanitize-payload.py --source github
```

## Smoke test

Runs webhook server + signed sample events. By default it starts a local mock OpenClaw service and verifies exactly one GitHub and one Linear event are forwarded after deduplication.

```bash
GITHUB_WEBHOOK_SECRET=... \
LINEAR_WEBHOOK_SECRET=... \
OPENCLAW_HOOKS_TOKEN=... \
scripts/smoke-test.sh
```

Use live OpenClaw instead of local mock:

```bash
GITHUB_WEBHOOK_SECRET=... \
LINEAR_WEBHOOK_SECRET=... \
OPENCLAW_HOOKS_TOKEN=... \
OPENCLAW_GATEWAY_URL=https://<your-openclaw-gateway> \
scripts/smoke-test.sh -l
```

## Basic validation before PR

```bash
bash -n scripts/*.sh
python3 -m py_compile scripts/sanitize-payload.py
```

Then run the smoke test command above.

## Security model (high level)

1. Verify webhook authenticity (GitHub/Linear signatures)
2. Extract only needed fields from payloads
3. Fence untrusted user text in explicit delimiters
4. Flag suspicious prompt-injection patterns
5. Deduplicate and throttle bursts before forwarding

See `references/payload-sanitization.md` for details.
