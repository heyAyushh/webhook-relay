# Tailscale Deployment

## Table of Contents
- [Architecture Options](#architecture-options)
- [Option A: Tailscale Funnel (Public Ingress)](#option-a-tailscale-funnel-public-ingress)
- [Option B: Private-Only (Tailnet Internal)](#option-b-private-only-tailnet-internal)
- [Option C: Hybrid (Funnel + MagicDNS)](#option-c-hybrid-funnel--magicdns)
- [Systemd Service](#systemd-service)
- [Docker Compose](#docker-compose)

## Architecture Options

| Scenario | Tailscale Feature | External Access |
|----------|-------------------|-----------------|
| **Preferred: colocated webhook + OpenClaw** | **Funnel + localhost** | Ingress only |
| GitHub/Linear need to reach webhook server | **Funnel** | Yes (public HTTPS) |
| Webhook server and OpenClaw on same tailnet | **MagicDNS** | No (private only) |
| Public ingress + remote OpenClaw | **Funnel + MagicDNS** | Ingress only |

## Option A: Tailscale Funnel (Public Ingress)

Funnel exposes a local port to the public internet via Tailscale's edge, providing automatic TLS.

```bash
# Expose webhook server on port 9000 via Funnel
tailscale funnel --bg 9000

# This creates a public URL like:
# https://your-machine.tail-net.ts.net/
# Configure this URL in GitHub/Linear webhook settings
```

Configure GitHub webhook URL: `https://your-machine.tail-net.ts.net/hooks/github-pr`
Configure Linear webhook URL: `https://your-machine.tail-net.ts.net/hooks/linear`

**Funnel requirements:**
- Enable Funnel in Tailscale admin ACLs: `"nodeAttrs": [{"target": ["*"], "attr": ["funnel"]}]`
- HTTPS only (automatic via Tailscale)
- Port 443 externally, proxied to your local port

### With custom port prefix

```bash
# If webhook uses -urlprefix webhooks instead of hooks:
tailscale funnel --bg 9000
# URL becomes: https://your-machine.tail-net.ts.net/webhooks/github-pr
```

## Option B: Private-Only (Tailnet Internal)

When both webhook server and event producers are on the same tailnet (e.g., self-hosted Git):

```bash
# webhook server binds to tailscale IP
webhook -hooks hooks.yaml -ip $(tailscale ip -4) -port 9000 -verbose

# Other tailnet nodes reach it via MagicDNS:
# http://webhook-server.tail-net.ts.net:9000/hooks/github-pr
```

For relay to OpenClaw (also on tailnet):
```bash
# In relay script, OPENCLAW_GATEWAY_URL uses MagicDNS:
export OPENCLAW_GATEWAY_URL="http://openclaw.tail-net.ts.net:3000"
```

## Option C: Funnel + Local OpenClaw (Preferred)

**Recommended.** Webhook server and OpenClaw run on the same host. Funnel provides public ingress for GitHub/Linear. Relay scripts call OpenClaw on localhost.

```
Internet                    Single host
──────────                  ───────────────────────────────
GitHub ──► Funnel ──► webhook server ──► localhost:3000 (OpenClaw)
Linear ──►
```

Setup:
```bash
# 1. Expose webhook server via Funnel for GitHub/Linear
tailscale funnel --bg 9000

# 2. Relay scripts call OpenClaw on localhost
export OPENCLAW_GATEWAY_URL="http://localhost:3000"
```

Benefits:
- Zero network hops for webhook -> OpenClaw relay (localhost)
- No need for MagicDNS resolution or cross-node WireGuard overhead
- GitHub/Linear reach webhook server over public internet (HTTPS via Funnel)
- No public exposure of OpenClaw gateway
- Simplest deployment — single machine, single `.env`

## Option D: Hybrid (Funnel + MagicDNS)

Use when OpenClaw runs on a **different** tailnet node from the webhook server.

```
Internet                    Tailnet
──────────                  ───────────────────────────────
GitHub ──► Funnel ──► webhook server ──► OpenClaw gateway
Linear ──►           (MagicDNS: webhook.ts.net)  (MagicDNS: openclaw.ts.net)
```

Setup:
```bash
# 1. Expose webhook server via Funnel for GitHub/Linear
tailscale funnel --bg 9000

# 2. Relay scripts use MagicDNS to reach OpenClaw on another node
export OPENCLAW_GATEWAY_URL="http://openclaw.tail-net.ts.net:3000"
```

Benefits:
- Webhook -> OpenClaw traffic encrypted via WireGuard
- No public exposure of OpenClaw gateway
- No manual TLS cert management

## Systemd Service

```ini
# /etc/systemd/system/webhook.service
[Unit]
Description=adnanh/webhook server
After=network.target tailscaled.service
Wants=tailscaled.service

[Service]
Type=simple
User=webhook
Group=webhook
ExecStart=/usr/local/bin/webhook \
  -hooks /opt/hooks/hooks.yaml \
  -port 9000 \
  -verbose \
  -hotreload
Restart=on-failure
RestartSec=5

# Environment
EnvironmentFile=/opt/hooks/.env

# Security hardening
NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/tmp/webhook-dedup /opt/hooks/scripts
ProtectHome=yes

[Install]
WantedBy=multi-user.target
```

```bash
# /opt/hooks/.env
GITHUB_WEBHOOK_SECRET=your-github-secret
LINEAR_WEBHOOK_SECRET=your-linear-secret
OPENCLAW_GATEWAY_URL=http://localhost:3000
OPENCLAW_HOOKS_TOKEN=your-openclaw-token
GITHUB_APP_ID=123456
GITHUB_APP_PRIVATE_KEY=/opt/hooks/secrets/github-app.pem
GITHUB_INSTALLATION_ID=78901234
LINEAR_AGENT_API_KEY=lin_api_xxxxx
LINEAR_AGENT_USER_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
```

## Docker Compose

```yaml
version: "3.8"
services:
  webhook:
    image: almir/webhook:latest
    volumes:
      - ./hooks.yaml:/etc/webhook/hooks.yaml:ro
      - ./scripts:/opt/hooks/scripts:ro
      - dedup-data:/tmp/webhook-dedup
    ports:
      - "9000:9000"
    environment:
      - GITHUB_WEBHOOK_SECRET
      - LINEAR_WEBHOOK_SECRET
      - OPENCLAW_GATEWAY_URL
      - OPENCLAW_HOOKS_TOKEN
    command: ["-hooks", "/etc/webhook/hooks.yaml", "-verbose", "-hotreload"]
    restart: unless-stopped
    # If using Tailscale sidecar:
    network_mode: "service:tailscale"

  tailscale:
    image: tailscale/tailscale:latest
    hostname: webhook
    environment:
      - TS_AUTHKEY=${TS_AUTHKEY}
      - TS_STATE_DIR=/var/lib/tailscale
      - TS_SERVE_CONFIG=/config/serve.json
    volumes:
      - ts-state:/var/lib/tailscale
      - ./ts-serve.json:/config/serve.json:ro
    cap_add:
      - NET_ADMIN
      - SYS_MODULE

volumes:
  dedup-data:
  ts-state:
```

```json
// ts-serve.json - Tailscale Funnel config
{
  "TCP": {
    "443": {
      "HTTPS": true
    }
  },
  "Web": {
    "${TS_CERT_DOMAIN}:443": {
      "Handlers": {
        "/": {
          "Proxy": "http://127.0.0.1:9000"
        }
      }
    }
  },
  "AllowFunnel": {
    "${TS_CERT_DOMAIN}:443": true
  }
}
```
