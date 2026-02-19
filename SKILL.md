---
name: webhook-relay
description: >
  Configure, deploy, and manage adnanh/webhook - a lightweight Go webhook server
  for receiving and routing HTTP webhooks. Use when setting up webhook endpoints,
  writing hooks.json/hooks.yaml configuration, verifying webhook signatures (GitHub
  HMAC, Linear), forwarding/relaying webhooks to downstream services, or deploying
  webhook servers with Tailscale. Covers: (1) Hook definitions with trigger rules,
  (2) GitHub and Linear webhook signature verification, (3) Relay patterns to
  OpenClaw or other agent gateways, (4) Tailscale integration for private networking
  and public ingress via Funnel, (5) Dedup/cooldown via relay scripts,
  (6) OpenClaw hooks.mappings config and transform modules,
  (7) Local development with gh webhook forward,
  (8) Interactive first-time setup wizard,
  (9) OpenClaw agent config (SOUL.md, HEARTBEAT.md, TOOLS.md, MEMORY.md, etc.).
---

# adnanh/webhook

Lightweight configurable webhook server written in Go. Binary: `webhook`.
Repo: https://github.com/adnanh/webhook

## Boot Verification (Do This First)

Before any configuration, deployment, or troubleshooting — verify the webhook Go server is installed and running. Read [references/boot.md](references/boot.md) and run the pre-flight checks. Everything else (hooks config, relay scripts, Tailscale, OpenClaw) depends on the server being up.

## First-Time Setup

If setting up for the first time, read [references/setup-wizard.md](references/setup-wizard.md) and walk the user through the interactive questionnaire. It collects infrastructure choices, credentials, and source selections, then generates all config files.

## Quick Start

```bash
# Install
brew install webhook
# or
go install github.com/adnanh/webhook@latest

# Run
webhook -hooks hooks.yaml -verbose -port 9000
```

Endpoints are created at `http://<host>:9000/hooks/{hook-id}`.

## Core Concept

webhook executes commands — it is **not** an HTTP proxy. To relay/forward webhooks to a downstream service (e.g. OpenClaw), use `execute-command` pointing to a script that calls `curl`.

## Hook Definition (Quick Reference)

```yaml
- id: my-hook                          # creates /hooks/my-hook
  execute-command: /path/to/script.sh
  command-working-directory: /opt/hooks
  response-message: accepted
  include-command-output-in-response: false
  http-methods: [POST]
  pass-environment-to-command:          # env vars from request
    - source: payload
      name: action
      envname: HOOK_ACTION
    - source: header
      name: X-GitHub-Event
      envname: GITHUB_EVENT
  pass-arguments-to-command:            # positional args from request
    - source: payload
      name: repository.full_name
  trigger-rule:                         # when to execute
    and:
      - match:
          type: payload-hmac-sha256
          secret: "{{ getenv "GITHUB_WEBHOOK_SECRET" }}"
          parameter:
            source: header
            name: X-Hub-Signature-256
      - match:
          type: value
          parameter:
            source: header
            name: X-GitHub-Event
          value: pull_request
```

### Parameter Sources

| Source | Syntax | Notes |
|--------|--------|-------|
| `payload` | `name: field.nested.0.value` | Dot-notation, 0-indexed arrays |
| `header` | `name: X-Header-Name` | HTTP header |
| `url` | `name: param` | Query string `?param=val` |
| `request` | `name: method` or `remote-addr` | Request metadata |
| `entire-payload` | — | Full body as JSON string |
| `entire-headers` | — | All headers as JSON string |

### Trigger Rule Match Types

| Type | Use |
|------|-----|
| `value` | Exact string match |
| `regex` | Go regexp pattern |
| `payload-hmac-sha256` | HMAC-SHA256 signature verification |
| `payload-hmac-sha1` | HMAC-SHA1 signature verification |
| `payload-hmac-sha512` | HMAC-SHA512 signature verification |
| `ip-whitelist` | CIDR range check |

Rules compose with `and`, `or`, `not` operators (nestable).

## Detailed References

- **Boot verification (pre-flight checks)**: [references/boot.md](references/boot.md)
- **Full hook fields & trigger rules**: [references/hook-definition.md](references/hook-definition.md)
- **GitHub PR/review hooks**: [references/github-hooks.md](references/github-hooks.md)
- **Linear issue/comment hooks**: [references/linear-hooks.md](references/linear-hooks.md)
- **OpenClaw relay pattern**: [references/openclaw-relay.md](references/openclaw-relay.md)
- **OpenClaw agent config (SOUL.md, HEARTBEAT.md, etc.)**: [references/openclaw-agents.md](references/openclaw-agents.md)
- **Payload sanitization (prompt injection defense)**: [references/payload-sanitization.md](references/payload-sanitization.md)
- **Tailscale deployment**: [references/tailscale.md](references/tailscale.md)

## File Organization

```
hooks/
├── hooks.yaml              # Hook definitions
└── scripts/
    ├── relay-github.sh     # GitHub -> sanitize -> OpenClaw relay
    ├── relay-linear.sh     # Linear -> sanitize -> OpenClaw relay
    ├── sanitize-payload.py # Prompt injection defense (allowlist + fence + flag)
    └── dedup.sh            # Shared dedup logic (Redis/file-based)
```
