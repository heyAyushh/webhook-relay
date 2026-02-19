# Linear Webhook Hooks

## Table of Contents
- [Agent Identity](#agent-identity)
- [Events](#events)
- [Hook Definition](#hook-definition)
- [Signature Verification](#signature-verification)
- [Relay Script](#relay-script)
- [Feedback Loop Prevention](#feedback-loop-prevention)

## Agent Identity

The OpenClaw `coder` agent operates on Linear as a **dedicated Linear user** (service account):

- Created by the workspace owner as a regular Linear user (e.g. `openclaw-bot@yourteam.com`)
- Joined to the owner's team so it can view/comment on team issues
- Has its own **personal API key** (`LINEAR_AGENT_API_KEY`) generated from the agent user's Linear settings
- Comments and updates made by the agent appear under this user's name in Linear

### Setup

1. Create a Linear account for the agent (use a shared/service email)
2. Owner invites the agent user to the workspace
3. Owner adds the agent user to the relevant team(s)
4. Log in as the agent user → Settings > API > Personal API keys → Create key
5. Store the key as `LINEAR_AGENT_API_KEY`

The agent user should have **Member** role (not Admin/Guest) — enough to read issues and post comments on joined teams, no workspace-level admin access.

## Events

Configure in Linear Settings > API > Webhooks (as the workspace owner, not the agent user):

| Event Type | Trigger |
|------------|---------|
| Issue | Created, updated, removed |
| Comment | Created, updated, removed |
| Project | Created, updated |
| Label | Created, updated |

Linear sends all subscribed events to a single URL with action metadata in the payload.

**Important:** The webhook subscription is workspace-level, not per-user. Events from all team members (including the agent user) are delivered. See [Feedback Loop Prevention](#feedback-loop-prevention).

## Signature Verification

Linear signs webhooks using HMAC-SHA256. The signature is in the `Linear-Signature` header (hex-encoded, no prefix — unlike GitHub's `sha256=` prefix).

**Note:** adnanh/webhook's `payload-hmac-sha256` match type expects the signature header value to start with `sha256=`. Linear does not include this prefix. Signature is verified in the relay script instead.

## Hook Definition

```yaml
- id: linear
  execute-command: /opt/hooks/scripts/relay-linear.sh
  command-working-directory: /opt/hooks
  response-message: accepted
  http-methods: [POST]
  include-command-output-in-response: false
  pass-environment-to-command:
    - source: header
      name: Linear-Signature
      envname: LINEAR_SIGNATURE
    - source: header
      name: Linear-Delivery
      envname: LINEAR_DELIVERY
    - source: payload
      name: type
      envname: LINEAR_EVENT_TYPE
    - source: payload
      name: action
      envname: LINEAR_ACTION
    - source: payload
      name: data.id
      envname: LINEAR_ENTITY_ID
    - source: payload
      name: data.team.key
      envname: LINEAR_TEAM_KEY
    - source: payload
      name: data.userId
      envname: LINEAR_ACTOR_ID
    - source: entire-payload
      envname: LINEAR_PAYLOAD
  # No trigger-rule HMAC check — signature verified in script
  # because Linear omits the "sha256=" prefix
```

Note: `data.userId` is extracted to identify the actor — used to skip events caused by the agent itself.

## Relay Script

Verifies signature and forwards sanitized payloads to OpenClaw `POST /hooks/agent?source=linear`. See [openclaw-relay.md](openclaw-relay.md) for how OpenClaw processes the payload (hook mappings, `linear.ts` transform, `coder` agent invocation).

```bash
#!/usr/bin/env bash
set -euo pipefail

# relay-linear.sh - Verify Linear signature and forward to OpenClaw
# Env vars: LINEAR_SIGNATURE, LINEAR_DELIVERY, LINEAR_EVENT_TYPE,
#           LINEAR_ACTION, LINEAR_ENTITY_ID, LINEAR_TEAM_KEY,
#           LINEAR_ACTOR_ID, LINEAR_PAYLOAD

# Verify HMAC-SHA256 signature
EXPECTED=$(echo -n "$LINEAR_PAYLOAD" | openssl dgst -sha256 -hmac "$LINEAR_WEBHOOK_SECRET" | sed 's/^.* //')
if [[ "$LINEAR_SIGNATURE" != "$EXPECTED" ]]; then
  echo "invalid signature"
  exit 1
fi

# Skip events caused by the agent itself (prevent feedback loops)
if [[ "${LINEAR_ACTOR_ID:-}" == "${LINEAR_AGENT_USER_ID}" ]]; then
  echo "skipping: event from agent user"
  exit 0
fi

GATEWAY_URL="${OPENCLAW_GATEWAY_URL}/hooks/agent?source=linear"
AUTH_TOKEN="${OPENCLAW_HOOKS_TOKEN}"

# Dedup
DEDUP_KEY="linear:${LINEAR_DELIVERY}:${LINEAR_ACTION}:${LINEAR_ENTITY_ID}"
DEDUP_DIR="/tmp/webhook-dedup"
mkdir -p "$DEDUP_DIR"

DEDUP_FILE="$DEDUP_DIR/$(echo -n "$DEDUP_KEY" | sha256sum | cut -d' ' -f1)"
if [[ -f "$DEDUP_FILE" ]]; then
  echo "duplicate: $DEDUP_KEY"
  exit 0
fi
touch "$DEDUP_FILE"
find "$DEDUP_DIR" -type f -mtime +7 -delete 2>/dev/null || true

# Sanitize payload before forwarding to LLM agent
SANITIZED=$(echo "$LINEAR_PAYLOAD" | python3 /opt/hooks/scripts/sanitize-payload.py --source linear --verbose)

# Forward sanitized payload to OpenClaw
curl -sf -X POST "$GATEWAY_URL" \
  -H "Authorization: Bearer ${AUTH_TOKEN}" \
  -H "Content-Type: application/json" \
  -H "X-Webhook-Source: linear" \
  -H "X-Linear-Event: ${LINEAR_EVENT_TYPE}" \
  -H "X-Linear-Delivery: ${LINEAR_DELIVERY}" \
  -d "$SANITIZED"
```

## Feedback Loop Prevention

The agent posts comments/updates to Linear using its own API key. Linear then fires a webhook for that event. Without protection, this creates an infinite loop: agent acts → webhook fires → agent acts → ...

**Two layers of protection:**

1. **Actor ID check in relay script** — compares `data.userId` from the event against `LINEAR_AGENT_USER_ID`. If the agent caused the event, the relay exits early before forwarding to OpenClaw.

2. **Bot sender check in OpenClaw transform** — the `linear.ts` transform should also check the actor and skip if it matches the agent user.

Both layers are needed because the relay check is the fast path (no OpenClaw call at all), while the transform check is defense-in-depth.
