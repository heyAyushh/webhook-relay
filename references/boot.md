# Boot Verification

Before configuring hooks, relay scripts, or Tailscale ingress, verify that the webhook Go server is installed and running. Everything else depends on it.

## Pre-Flight Checks

Run in order. Stop at the first failure.

### 1. Webhook server (required gate)

Nothing works without the Go server. Verify these before anything else:

```bash
# Binary installed?
command -v webhook >/dev/null 2>&1 && echo "OK: $(command -v webhook)" || echo "FAIL: not installed"

# Install if missing:
# brew install webhook
# go install github.com/adnanh/webhook@latest

# hooks.yaml valid?
python3 -c "import yaml; yaml.safe_load(open('hooks.yaml'))" 2>&1 \
  && echo "OK: valid YAML" || echo "FAIL: invalid YAML"

# Server starts and responds?
webhook -hooks hooks.yaml -verbose -port 9000 &
PID=$!; sleep 2
if kill -0 "$PID" 2>/dev/null; then
  CODE=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:9000/hooks/github-pr 2>/dev/null)
  echo "OK: server responding (HTTP $CODE)"
  kill "$PID"
else
  echo "FAIL: server exited — check hooks.yaml for errors"
fi
```

A 405 on GET is normal for POST-only hooks — it confirms the server is up and the hook ID is registered. Connection refused means the server isn't running.

### 2. Relay stack (after server confirmed)

These verify the relay layer between webhook server and OpenClaw:

```bash
# Env vars loaded?
for var in GITHUB_WEBHOOK_SECRET LINEAR_WEBHOOK_SECRET OPENCLAW_GATEWAY_URL OPENCLAW_HOOKS_TOKEN; do
  [ -n "${!var:-}" ] && echo "OK: $var" || echo "FAIL: $var not set"
done

# Relay scripts executable?
for s in scripts/relay-github.sh scripts/relay-linear.sh; do
  [ -x "$s" ] && echo "OK: $s" || echo "WARN: $s not executable (chmod +x)"
done

# Sanitizer works?
echo '{"action":"opened"}' | python3 scripts/sanitize-payload.py --source github >/dev/null 2>&1 \
  && echo "OK: sanitizer" || echo "FAIL: sanitizer"
```

### 3. OpenClaw reachable (after relay confirmed)

The relay scripts forward to OpenClaw. Verify it's running:

```bash
GATEWAY="${OPENCLAW_GATEWAY_URL:-http://localhost:3000}"
curl -sf "$GATEWAY/health" >/dev/null 2>&1 \
  && echo "OK: OpenClaw at $GATEWAY" \
  || echo "FAIL: OpenClaw not reachable at $GATEWAY"
```

See [openclaw-relay.md](openclaw-relay.md) for OpenClaw hook configuration, mappings, and transform modules.

## Quick Boot Script

```bash
#!/usr/bin/env bash
set -euo pipefail
FAIL=0
check() { eval "$2" >/dev/null 2>&1 && echo "  OK: $1" || { echo "FAIL: $1"; FAIL=1; }; }

echo "=== Webhook Server ==="
check "webhook binary"      "command -v webhook"
check "hooks.yaml exists"   "[ -f hooks.yaml ]"
check "hooks.yaml valid"    "python3 -c \"import yaml; yaml.safe_load(open('hooks.yaml'))\""

echo "=== Relay Stack ==="
check "relay-github.sh"     "[ -x scripts/relay-github.sh ]"
check "relay-linear.sh"     "[ -x scripts/relay-linear.sh ]"
check "sanitizer"            "echo '{}' | python3 scripts/sanitize-payload.py --source github"
check "GITHUB_WEBHOOK_SECRET" "[ -n \"\${GITHUB_WEBHOOK_SECRET:-}\" ]"
check "OPENCLAW_GATEWAY_URL"  "[ -n \"\${OPENCLAW_GATEWAY_URL:-}\" ]"

echo "=== OpenClaw ==="
check "OpenClaw reachable"  "curl -sf ${OPENCLAW_GATEWAY_URL:-http://localhost:3000}/health"

[ $FAIL -eq 0 ] && echo "All checks passed." || echo "Fix failures above before starting."
exit $FAIL
```

## Order of Operations

1. **Boot check** — verify webhook binary, config, env vars, OpenClaw
2. **Start webhook server** — `webhook -hooks hooks.yaml -verbose -port 9000`
3. **Expose via Tailscale** — `tailscale funnel --bg 9000` (see [tailscale.md](tailscale.md))
4. **Verify end-to-end** — send test payload (see [openclaw-relay.md](openclaw-relay.md#end-to-end-test))

Do not expose via Tailscale or configure GitHub/Linear webhook URLs until the server is confirmed running locally.
