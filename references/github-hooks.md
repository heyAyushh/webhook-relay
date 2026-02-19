# GitHub Webhook Hooks

## Table of Contents
- [Agent Identity](#agent-identity)
- [GitHub App Setup](#github-app-setup)
- [URL Model (Single-Port Hardening)](#url-model-single-port-hardening)
- [Required Inputs](#required-inputs)
- [Create the App via gh CLI (Manifest)](#create-the-app-via-gh-cli-manifest)
- [Create the App in Web UI (Fallback)](#create-the-app-in-web-ui-fallback)
- [Events to Subscribe](#events-to-subscribe)
- [Webhook Delivery Levels](#webhook-delivery-levels)
- [Hook Definition](#hook-definition)
- [Relay Script](#relay-script)
- [Feedback Loop Prevention](#feedback-loop-prevention)
- [Authentication: App vs PAT](#authentication-app-vs-pat)
- [Local Development](#local-development)
- [Fallback: Per-Repo Webhook](#fallback-per-repo-webhook)

## Agent Identity

The OpenClaw `coder` agent operates on GitHub as a **GitHub App bot**:

- The App has its own bot identity: `your-app-name[bot]`
- Reviews, comments, and status checks posted by the agent appear under this bot name
- No separate machine user account needed — the App *is* the identity
- The `[bot]` suffix is automatic and cannot be removed (GitHub enforces it)
- The App is created by the repo/org owner and installed on their account

This differs from Linear where a separate user account is needed. On GitHub, the App itself provides the bot identity, permissions, webhook delivery, and auth — all in one.

## GitHub App Setup

The GitHub App is both the webhook delivery mechanism and the agent's identity. It:
- Covers all repos it's installed on (current and future) with a single webhook URL
- Works on personal accounts and orgs — no org-level webhook needed
- Provides fine-grained permissions (not a broad PAT)
- Uses installation access tokens that auto-expire (1 hour)
- Has its own bot identity: `app-name[bot]`

### URL Model (Single-Port Hardening)

Use one public relay base URL and keep OpenClaw private:
- Public ingress only to `adnanh/webhook` (single exposed service)
- Keep OpenClaw on `localhost` or private tailnet interface (not internet-facing)
- Use path-based routes on the same host:
  - `https://<relay-host>/hooks/github-pr`
  - `https://<relay-host>/hooks/linear`
  - Manifest setup redirect (CLI flow): `https://<relay-host>/auth/github/manifest`
  - Optional OAuth callback only if needed: `https://<relay-host>/auth/github/callback`

`Webhook URL` receives GitHub webhook deliveries. `Callback URL` is only for OAuth redirects and should be omitted unless OAuth login is required.

### Required Inputs

Collect these values before creating the app:

| Input | Required | Notes |
|------|----------|-------|
| App owner (personal/org) | Yes | Determines where the app is created |
| App name | Yes | Bot identity becomes `name[bot]` |
| Homepage URL | Yes | Project/app URL |
| Webhook URL | Yes | Usually `https://<relay-host>/hooks/github-pr` |
| Webhook secret | Yes | `GITHUB_WEBHOOK_SECRET` |
| Permissions | Yes | Default: PR `write`, contents `read`, metadata `read` |
| Events | Yes | See [Events to Subscribe](#events-to-subscribe) |
| App visibility (`public`) | Yes | `false` for private/internal by default |
| Manifest redirect URL | Yes (CLI flow) | Receives temporary `code` used by `/app-manifests/{code}/conversions` |
| OAuth callback URLs | No | Set only when OAuth login flow is required |
| Install scope (all vs selected repos) | Yes | Security choice; selected repos is tighter |

### Create the App via gh CLI (Manifest)

GitHub CLI has no `gh app create` subcommand. Use `gh api` with the GitHub App manifest flow:

```bash
# 0) Authenticate gh
gh auth status || gh auth login -h github.com

# 1) Set required values
APP_NAME="openclaw-coder"
APP_HOMEPAGE_URL="https://example.com"
RELAY_BASE_URL="https://your-machine.tail-net.ts.net"
WEBHOOK_URL="${RELAY_BASE_URL}/hooks/github-pr"
WEBHOOK_SECRET="${GITHUB_WEBHOOK_SECRET}"
MANIFEST_REDIRECT_URL="${RELAY_BASE_URL}/auth/github/manifest"
APP_PUBLIC=false

# Optional: set only if OAuth login is required
CALLBACK_URL=""

# Optional: set org slug if creating app under an org
APP_OWNER_ORG=""

MANIFEST_PATH="/tmp/github-app-manifest.json"

# 2) Build manifest JSON (omit callback_urls unless OAuth is needed)
if [[ -n "${CALLBACK_URL}" ]]; then
  cat >"${MANIFEST_PATH}" <<JSON
{
  "name": "${APP_NAME}",
  "url": "${APP_HOMEPAGE_URL}",
  "public": ${APP_PUBLIC},
  "webhook_secret": "${WEBHOOK_SECRET}",
  "redirect_url": "${MANIFEST_REDIRECT_URL}",
  "hook_attributes": {"url": "${WEBHOOK_URL}", "active": true},
  "callback_urls": ["${CALLBACK_URL}"],
  "default_permissions": {
    "pull_requests": "write",
    "contents": "read",
    "metadata": "read"
  },
  "default_events": [
    "pull_request",
    "pull_request_review",
    "pull_request_review_comment",
    "pull_request_review_thread",
    "issue_comment"
  ]
}
JSON
else
  cat >"${MANIFEST_PATH}" <<JSON
{
  "name": "${APP_NAME}",
  "url": "${APP_HOMEPAGE_URL}",
  "public": ${APP_PUBLIC},
  "webhook_secret": "${WEBHOOK_SECRET}",
  "redirect_url": "${MANIFEST_REDIRECT_URL}",
  "hook_attributes": {"url": "${WEBHOOK_URL}", "active": true},
  "default_permissions": {
    "pull_requests": "write",
    "contents": "read",
    "metadata": "read"
  },
  "default_events": [
    "pull_request",
    "pull_request_review",
    "pull_request_review_comment",
    "pull_request_review_thread",
    "issue_comment"
  ]
}
JSON
fi

# 3) Open manifest registration URL
STATE="$(openssl rand -hex 16)"
ENCODED_MANIFEST="$(jq -Rs @uri < "${MANIFEST_PATH}")"
MANIFEST_BASE_URL="https://github.com/settings/apps/new"
if [[ -n "${APP_OWNER_ORG}" ]]; then
  MANIFEST_BASE_URL="https://github.com/organizations/${APP_OWNER_ORG}/settings/apps/new"
fi
MANIFEST_URL="${MANIFEST_BASE_URL}?state=${STATE}&manifest=${ENCODED_MANIFEST}"
echo "${MANIFEST_URL}"

# macOS convenience:
open "${MANIFEST_URL}"

# 4) After browser flow, copy ?code=<...> from manifest redirect URL and convert
CODE="paste_manifest_code_here"
gh api --method POST "/app-manifests/${CODE}/conversions" > /tmp/github-app-created.json

# 5) Inspect returned fields and store app metadata
jq -r '.id' /tmp/github-app-created.json
```

After conversion, collect/store:
- App ID (`GITHUB_APP_ID`)
- Client ID / Client Secret (only needed if OAuth flow is enabled)
- PEM private key (`GITHUB_APP_PRIVATE_KEY`)

### Create the App in Web UI (Fallback)

1. Go to **Settings > Developer settings > GitHub Apps > New GitHub App**
2. Set:
   - **App name**: `openclaw-coder` (or your preferred name)
   - **Homepage URL**: your project URL
   - **Webhook URL**: `https://<relay-host>/hooks/github-pr`
   - **Webhook secret**: value of `GITHUB_WEBHOOK_SECRET`
   - **Permissions**:
     - Pull requests: **Read & write** (post reviews, comments)
     - Contents: **Read-only** (read PR diffs)
     - Metadata: **Read-only** (required)
   - **Subscribe to events**: see [Events to Subscribe](#events-to-subscribe)
   - **Callback URL**: leave empty unless OAuth login is required
3. Create the app
4. Generate a **private key** (downloads `.pem` file) — store securely
5. Note the **App ID** and **Installation ID** (shown after installing)

### Install the App

- **Personal account**: Settings > Applications > Install your app > select repos (or all)
- **Organization**: Org Settings > Installed GitHub Apps > Install > select repos (or all)

### Auth Flow for Outbound API Calls

The GitHub App authenticates outbound API calls (posting reviews) using short-lived installation tokens instead of a long-lived PAT:

```bash
# 1. Create JWT from App ID + private key (expires 10 min)
JWT=$(python3 -c "
import jwt, time
now = int(time.time())
payload = {'iat': now - 60, 'exp': now + 600, 'iss': $GITHUB_APP_ID}
print(jwt.encode(payload, open('$GITHUB_APP_PRIVATE_KEY').read(), algorithm='RS256'))
")

# 2. Exchange JWT for installation access token (expires 1 hour)
TOKEN=$(curl -s -X POST \
  -H "Authorization: Bearer $JWT" \
  -H "Accept: application/vnd.github+json" \
  "https://api.github.com/app/installations/$GITHUB_INSTALLATION_ID/access_tokens" \
  | jq -r .token)

# 3. Use token for API calls
curl -H "Authorization: token $TOKEN" \
  https://api.github.com/repos/owner/repo/pulls/1/reviews
```

OpenClaw agents should use this flow (or a library like `octokit` with app auth) instead of a static PAT.

## Events to Subscribe

Configure in GitHub App settings under **Subscribe to events**:

| Event | Use |
|-------|-----|
| `pull_request` | PR opened, closed, synchronized, labeled |
| `pull_request_review` | Review submitted, dismissed |
| `pull_request_review_comment` | Inline review comments |
| `pull_request_review_thread` | Thread resolved/unresolved |
| `issue_comment` | PR conversation comments (slash commands) |

These are the same events whether using a GitHub App or per-repo webhook. The payloads are identical.

## Webhook Delivery Levels

| Level | Setup | Scope | Best for |
|-------|-------|-------|----------|
| **GitHub App** | Developer settings > GitHub Apps | All installed repos | Primary approach — works on personal + org |
| **Organization** | Org Settings > Webhooks | All org repos | Org-only, no personal repos |
| **Repository** | Repo Settings > Webhooks | Single repo | Testing, one-off repos |

GitHub App is recommended because it's the only option that covers both personal and org repos with a single webhook URL.

## Hook Definition

The adnanh/webhook hook definition is the same regardless of which GitHub delivery level sends events. The payload format is identical.

```yaml
- id: github-pr
  execute-command: /opt/hooks/scripts/relay-github.sh
  command-working-directory: /opt/hooks
  response-message: accepted
  http-methods: [POST]
  include-command-output-in-response: false
  pass-environment-to-command:
    - source: header
      name: X-GitHub-Event
      envname: GITHUB_EVENT
    - source: header
      name: X-GitHub-Delivery
      envname: GITHUB_DELIVERY
    - source: payload
      name: action
      envname: GITHUB_ACTION
    - source: payload
      name: repository.full_name
      envname: GITHUB_REPO
    - source: payload
      name: pull_request.number
      envname: GITHUB_PR_NUMBER
    - source: payload
      name: sender.login
      envname: GITHUB_SENDER
    - source: payload
      name: installation.id
      envname: GITHUB_INSTALLATION_ID
    - source: entire-payload
      envname: GITHUB_PAYLOAD
  trigger-rule:
    and:
      - match:
          type: payload-hmac-sha256
          secret: "{{ getenv \"GITHUB_WEBHOOK_SECRET\" }}"
          parameter:
            source: header
            name: X-Hub-Signature-256
      - or:
          - match:
              type: value
              value: pull_request
              parameter:
                source: header
                name: X-GitHub-Event
          - match:
              type: value
              value: pull_request_review
              parameter:
                source: header
                name: X-GitHub-Event
          - match:
              type: value
              value: pull_request_review_comment
              parameter:
                source: header
                name: X-GitHub-Event
          - match:
              type: value
              value: issue_comment
              parameter:
                source: header
                name: X-GitHub-Event
```

Note: GitHub App payloads include an `installation.id` field. This is extracted so the relay/agent can mint installation tokens for the correct installation.

### Filtering Non-Actionable Events

```yaml
- match:
    type: regex
    regex: "^(opened|synchronize|reopened|submitted|created)$"
    parameter:
      source: payload
      name: action
```

## Relay Script

Forwards sanitized payloads to OpenClaw `POST /hooks/agent?source=github-pr`. See [openclaw-relay.md](openclaw-relay.md) for how OpenClaw processes the payload (hook mappings, `github-pr.ts` transform, `coder` agent invocation).

```bash
#!/usr/bin/env bash
set -euo pipefail

# relay-github.sh - Forward GitHub webhook to OpenClaw gateway
# Env vars set by webhook via pass-environment-to-command:
#   GITHUB_EVENT, GITHUB_DELIVERY, GITHUB_ACTION, GITHUB_REPO,
#   GITHUB_PR_NUMBER, GITHUB_SENDER, GITHUB_INSTALLATION_ID, GITHUB_PAYLOAD

GATEWAY_URL="${OPENCLAW_GATEWAY_URL}/hooks/agent?source=github-pr"
AUTH_TOKEN="${OPENCLAW_HOOKS_TOKEN}"

# Skip events caused by the App bot itself (prevent feedback loops)
# GitHub App bot sender login is always "app-name[bot]"
if [[ "${GITHUB_SENDER}" == *"[bot]" ]]; then
  echo "skipping: event from bot ${GITHUB_SENDER}"
  exit 0
fi

# Dedup: skip if already processed
DEDUP_KEY="github:${GITHUB_DELIVERY}:${GITHUB_ACTION}:${GITHUB_PR_NUMBER}"
DEDUP_DIR="/tmp/webhook-dedup"
mkdir -p "$DEDUP_DIR"

DEDUP_FILE="$DEDUP_DIR/$(echo -n "$DEDUP_KEY" | sha256sum | cut -d' ' -f1)"
if [[ -f "$DEDUP_FILE" ]]; then
  echo "duplicate: $DEDUP_KEY"
  exit 0
fi
touch "$DEDUP_FILE"
# Clean entries older than 7 days
find "$DEDUP_DIR" -type f -mtime +7 -delete 2>/dev/null || true

# Cooldown: 1 event per PR per 30s
COOLDOWN_FILE="$DEDUP_DIR/cooldown-${GITHUB_REPO//\//-}-${GITHUB_PR_NUMBER}"
if [[ -f "$COOLDOWN_FILE" ]]; then
  AGE=$(( $(date +%s) - $(stat -f%m "$COOLDOWN_FILE" 2>/dev/null || stat -c%Y "$COOLDOWN_FILE") ))
  if (( AGE < 30 )); then
    echo "cooldown active for PR #${GITHUB_PR_NUMBER}"
    exit 0
  fi
fi
touch "$COOLDOWN_FILE"

# Sanitize payload before forwarding to LLM agent
SANITIZED=$(echo "$GITHUB_PAYLOAD" | python3 /opt/hooks/scripts/sanitize-payload.py --source github --verbose)

# Forward sanitized payload to OpenClaw
curl -sf -X POST "$GATEWAY_URL" \
  -H "Authorization: Bearer ${AUTH_TOKEN}" \
  -H "Content-Type: application/json" \
  -H "X-Webhook-Source: github" \
  -H "X-GitHub-Event: ${GITHUB_EVENT}" \
  -H "X-GitHub-Delivery: ${GITHUB_DELIVERY}" \
  -H "X-GitHub-Installation: ${GITHUB_INSTALLATION_ID}" \
  -d "$SANITIZED"
```

## Feedback Loop Prevention

The agent posts reviews/comments via the GitHub App. GitHub then fires a webhook for that event. Without protection, this creates an infinite loop.

**GitHub makes this easy:** App bot actions set `sender.login` to `app-name[bot]`. The relay script checks for the `[bot]` suffix and exits early.

```bash
# In relay-github.sh
if [[ "${GITHUB_SENDER}" == *"[bot]" ]]; then
  echo "skipping: event from bot ${GITHUB_SENDER}"
  exit 0
fi
```

**Two layers of protection:**

1. **`[bot]` suffix check in relay script** — catches all bot senders, not just your app. Fast path — no OpenClaw call at all.
2. **Bot sender check in OpenClaw transform** — the `github-pr.ts` transform also checks `sender.login.endsWith('[bot]')` as defense-in-depth.

This is simpler than Linear's approach because GitHub automatically tags App actions with `[bot]`, whereas Linear uses the actor's user ID which must be configured explicitly.

## Authentication: App vs PAT

| | GitHub App | Machine User PAT |
|-|-----------|-----------------|
| **Scope** | Fine-grained per-permission | Broad or fine-grained PAT scopes |
| **Token lifetime** | 1 hour (auto-expire) | Until revoked or expired |
| **Identity** | Bot: `app-name[bot]` | Real user account |
| **Multi-repo** | Single app covers all installed repos | Single token covers all accessible repos |
| **Rate limits** | Higher (5000/hr per installation) | Standard (5000/hr per user) |
| **Setup** | More initial config (JWT + key) | Simpler (generate token) |

**Recommendation**: Use GitHub App for production. Use a PAT only for quick local testing.

## Local Development

Use `gh webhook forward` to tunnel GitHub App events to a local webhook server:

```bash
# Terminal 1: run webhook server
webhook -hooks hooks.yaml -verbose -port 9000

# Terminal 2: forward events from your GitHub App to local server
gh webhook forward \
  --events=pull_request,pull_request_review,pull_request_review_comment \
  --url=http://localhost:9000/hooks/github-pr

# Or for a specific repo (repo-level webhook, not app):
gh webhook forward \
  --repo=owner/repo \
  --events=pull_request,pull_request_review,pull_request_review_comment \
  --url=http://localhost:9000/hooks/github-pr
```

## Fallback: Per-Repo Webhook

If you can't create a GitHub App (e.g. restrictions on the org), add webhooks per-repo:

1. Repo Settings > Webhooks > Add webhook
2. Payload URL: `https://your-machine.tail-net.ts.net/hooks/github-pr`
3. Content type: `application/json`
4. Secret: value of `GITHUB_WEBHOOK_SECRET`
5. Select events (same list as above)

The hook definition and relay script are identical — only the GitHub-side setup differs.
